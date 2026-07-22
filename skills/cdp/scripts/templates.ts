#!/usr/bin/env bun
/**
 * templates.ts — Pure JS code generators for CDP browser automation
 *
 * ALL functions are PURE: same inputs → same outputs. No I/O, no side effects.
 * Every function returns a JavaScript source code string that can be piped to
 * browser-harness-js for execution in a CDP-connected REPL.
 *
 * DESIGN:
 *   - GENERIC: URL pattern matching detects PDFs by (/\/pdf\/|\.pdf$/i),
 *     not hardcoded site names.
 *   - TESTABLE: every template function can be unit tested by checking
 *     its output string.
 *   - MEMORY SAFE: every tab creation has a corresponding close in try/finally.
 *   - TIMEOUT EVERYWHERE: every per-tab operation uses Promise.race with
 *     configurable timeout (default 15s).
 *   - PERFORMANT: uses Runtime.evaluate for all DOM extraction.
 *     Switched from callFunctionOn (5-10x faster for DOM queries
 *     but has executionContextId serialization bugs).
 *
 * DEPENDENCIES:
 *   - qualityGateCode from ./quality — embedded in batchHarvestCode
 */

import { qualityGateCode } from './quality';

// ─── Google Search DOM Selectors ─────────────────────────────────────────
// These extract results from Google SERP pages. Known limitation: not generic.
// Site-specific selectors that may need updating if Google changes its markup.

// Google SERP extraction: uses attribute-based selector (a[href] filtered by hostname)
// instead of brittle CSS class names. This is resilient to Google's class name rotation.
const GOOGLE_RESULT_CONTAINER = 'data-hveid';
const GOOGLE_TRANSLATE_PATTERN =
  /D\u1ECBch trang n\u00E0y|B\u1EA3n d\u1ECBch trang n\u00E0y/;

// ─── Connect Code ────────────────────────────────────────────────────────

/**
 * connectCode: Pure — returns JS to connect to CDP on given port.
 * No side effects. Given same port, always returns same string.
 */
export function connectCode(port: number): string {
  return (
    'if(!session.isConnected()){try{await session.connect({port:' +
    port +
    "})}catch(e){throw new Error(\"Cannot connect on port " +
    port +
    '")}}'
  );
}

// ─── Extraction ────────────────────────────────────────────────────────────
// Uses Runtime.evaluate with IIFE-wrapped function declarations for DOM
// extraction. The extractionCode() returns a pure function declaration;
// callers wrap it with an IIFE for use with Runtime.evaluate.
// This avoids the executionContextId serialization bug in callFunctionOn.

/**
 * extractionCode: Pure — returns an arrow function declaration for DOM extraction.
 * Used with Runtime.evaluate by wrapping as an IIFE (caller bakes arguments
 * into the expression string). Compatible with callFunctionOn for legacy use.
 *
 * Single CDP round-trip for extraction + quality gate + section boundary
 * detection. Executes directly in V8 context.
 *
 * @returns A function declaration string taking (offset, maxLen, url, selector)
 *          and returning JSON with content, total_length, truncated flag,
 *          sections, and quality gate error flags.
 */
export function extractionCode(): string {
  return `(offset, maxLen, url, selector) => {
    const sel = selector || 'article, main, [role=main]';
    const root = document.querySelector(sel) || document.body;
    if(!root) return JSON.stringify({url,content:"",total_length:0,returned_length:0,offset,truncated:false,sections:[],_error:"no_content_root"});
    const fullText=root.innerText||root.textContent||"";
    const totalLen=fullText.length;
    const off=offset<0?0:offset;
    const end=maxLen<0?totalLen:Math.min(off+maxLen,totalLen);
    const content=off<totalLen?fullText.slice(off,end):"";
    const isJunk=off===0&&(content.length<80||
      /This site can't be reached/i.test(content)||
      /ERR_CONNECTION/i.test(content)||
      /404 Not Found/i.test(content)||
      /^\\s*$/.test(content)||
      content==="Read More \\u00bb"||
      (content.length<100&&content.indexOf("\\"")<0));
    const headings=root.querySelectorAll("h1,h2,h3");
    const sections=[];
    for(var i=0;i<headings.length;i++){
      var h=headings[i];
      var hText=(h.innerText||"").trim();
      if(hText&&hText.length>1){
        var idx=fullText.indexOf(hText);
        if(idx>=0) sections.push({heading:hText,level:parseInt(h.tagName[1]),offset:idx});
      }
    }
    return JSON.stringify({
      url,
      content:isJunk?"":content,
      _error:isJunk?"low_quality_content":undefined,
      total_length:totalLen,
      returned_length:content.length,
      offset:off,
      truncated:maxLen>=0&&totalLen>(off+maxLen),
      sections
    });
  }`;
}

// ─── ReadyState Polling Snippets ─────────────────────────────────────────
// Reusable snippets for multi-tab readyState polling.
// Uses Runtime.callFunctionOn instead of evaluate for faster V8 execution.

/**
 * readyStatePoll: Pure — returns JS snippet that polls document.readyState
 * using callFunctionOn. Used in operations where session.waitFor is unreliable
 * (multi-tab switching orphans waitFor listeners per CDP skill Trap 2).
 */
export function readyStatePoll(): string {
  return [
    'for(var i=0;i<50;i++){',
    'try{var rs=await session.Runtime.evaluate({',
    '  expression:"document.readyState",',
    '  returnByValue:true',
    '});',
    'if(rs.result&&rs.result.value==="complete")break;',
    'if(rs.result&&rs.result.value==="interactive"&&i>15)break;',
    '}catch(ex){}',
    'await new Promise(function(r){setTimeout(r,200);});',
    '}',
  ].join('');
}

/**
 * readyStateCheck: Pure — returns JS expression for a single readyState check
 * using callFunctionOn (lighter than the full poll loop).
 */
export function readyStateCheck(): string {
  return 'await session.Runtime.evaluate({expression:"document.readyState",returnByValue:true})';
}

// ─── Full Command JS Generators ──────────────────────────────────────────

/**
 * followCode: Pure — returns full JS program for single-URL follow.
 * Input: url, offset, maxLen, timeout, selector, port.
 * Output: JS string that connects, creates one tab, navigates, extracts
 *         via Runtime.evaluate with IIFE-wrapped extraction function,
 *         closes tab, and returns result JSON.
 */
export function followCode(
  url: string,
  offset: number,
  maxLen: number,
  timeout: number,
  selector: string,
  port: number,
): string {
  const urlJson = JSON.stringify(url);
  const sel = JSON.stringify(selector);
  const extractFn = extractionCode();

  return [
    'return (async function(){',
    'var errors=[];var loadError=null;',
    `if(!session.isConnected()){try{await session.connect({port:${port}})}catch(e){throw new Error("CDP connect: "+e.message);}}`,
    // Create tab with about:blank — no navigation yet
    `var {targetId, sessionId}=await session.createTarget({url:"about:blank",background:true});`,
    'try{',
    'await session.use(targetId);',
    'await session.Page.enable();',
    'await session.Page.setLifecycleEventsEnabled({enabled:true});',
    // Register listener BEFORE navigating (CAN'T miss the event)
    `var navPromise=session.waitFor("Page.lifecycleEvent",function(p){return p.name==="networkIdle";},${timeout});`,
    // Single navigation — events already flowing
    `await session.Page.navigate({url:${urlJson}});`,
    'try{await navPromise;}catch(e){loadError="nav_timeout: "+e.message;}',
    // Fallback: poll readyState if waitFor timed out (Trap 2: multi-tab orphans waitFor)
    'if(loadError){' + readyStatePoll() + '}',
    // Structured extraction via Runtime.evaluate — args baked into expression
    `var result={};`,
    `try{`,
    `var r=await session.Runtime.evaluate({`,
    `  expression:${JSON.stringify(`(${extractFn})(${offset}, ${maxLen}, ${urlJson}, ${sel})`)},`,
    `  returnByValue:true`,
    `});`,
    `result=JSON.parse((r.result&&r.result.value)||"{}");`,
    'if(result._error){loadError=result._error;}',
    '}catch(e){loadError=loadError||"extraction_failed: "+e.message;}',
    'if(loadError){result._error=result._error||loadError;}',
    `return JSON.stringify(result);`,
    '}finally{',
    'session.closeTab(targetId,sessionId).catch(function(){});',
    '}',
    '})()',
  ].join('\n');
}

/**
 * batchFollowCode: Pure — returns full JS for batch-URL follow.
 * Creates N tabs in parallel (Promise.all), navigates all, extracts with
 * per-tab timeout and error isolation (Promise.allSettled pattern),
 * closes all in parallel, returns JSON array.
 */
export function batchFollowCode(
  urls: string[],
  selector: string,
  timeoutMs: number,
  port: number,
  offset: number,
  maxLen: number,
): string {
  const conn = connectCode(port);
  const urlsJson = JSON.stringify(urls);
  const selJson = JSON.stringify(selector);
  const extractFn = extractionCode();

  // JS snippet for per-tab extraction (uses `tab`, `selector`, `timeoutMs` in scope)
  // Uses Runtime.evaluate with IIFE-wrapped extraction function
  const extractTabCode = [
    'await session.use(tab.targetId);',
    'let content="";let loadError=null;',
    'try{await Promise.race([',
    '(async()=>{',
    'if(tab.type==="pdf"){',
    'await new Promise(r=>setTimeout(r,3000));',
    'try{const r=await session.Runtime.evaluate({',
    '  expression:"document.body?.innerText||document.querySelector(\\"embed\\")?.shadowRoot?.textContent||\\"\\"",',
    '  returnByValue:true});',
    'content=(r.result&&r.result.value)||"";',
    'let totalLen=content.length;',
    'let sections=[];',
    'let truncated=maxLen>0&&totalLen>(offset+maxLen);',
    'var dataObj={url:tab.originalUrl||tab.fetchUrl,content:content,truncated,total_length:totalLen,returned_length:content.length,offset,sections:sections};',
    'results.push(dataObj);',
    '}catch(e){loadError="pdf_extraction_failed: "+e.message;results.push({url:tab.originalUrl||tab.fetchUrl,content:"",_error:loadError});}',
    '}else{',
    // Poll readyState before extraction (Trap 2: don't use waitFor in multi-tab)
    'for(var i=0;i<50;i++){',
    'try{var rs=await session.Runtime.evaluate({',
    '  expression:"document.readyState",',
    '  returnByValue:true',
    '});',
    'if(rs.result&&rs.result.value==="complete")break;',
    'if(rs.result&&rs.result.value==="interactive"&&i>15)break;',
    '}catch(ex){}',
    'await new Promise(function(r){setTimeout(r,200);});',
    '}',
    // Structured extraction via Runtime.evaluate — args baked into expression
    'var r=await session.Runtime.evaluate({',
    '  expression:"(" + ' + JSON.stringify(extractFn) + ' + ")(" + offset + "," + maxLen + "," + JSON.stringify(tab.fetchUrl) + "," + JSON.stringify(selector) + ")",',
    '  returnByValue:true',
    '});',
    'var parsed=JSON.parse((r.result&&r.result.value)||"{}");',
    'var dataObj={url:tab.originalUrl||tab.fetchUrl,content:parsed.content||"",total_length:parsed.total_length||0,returned_length:parsed.returned_length||0,offset:parsed.offset||0,truncated:parsed.truncated||false,sections:parsed.sections||[]};',
    'if(parsed._error)dataObj._error=parsed._error;',
    'results.push(dataObj);',
    '}',
    '})(),',
    'new Promise((_,reject)=>setTimeout(()=>reject(new Error("tab_timeout")),timeoutMs))',
    ']);}catch(e){loadError=e.message;}',
  ].join('');

  return [
    conn,
    'const urls=' + urlsJson + ';',
    'const selector=' + selJson + ';',
    'const timeoutMs=' + timeoutMs + ';',
    'var offset=' + offset + ';',
    'var maxLen=' + maxLen + ';',
    'const processed=urls.map(function(url){',
    'const pdfMatch=url.match(/(\\/pdf\\/|\\.pdf$)/i);',
    'if(pdfMatch&&pdfMatch[1]==="/pdf/"){',
    'const absUrl=url.replace(/\\/pdf\\/(.+)/,"/abs/$1").replace(/\\.pdf$/,"");',
    'if(absUrl!==url){return{originalUrl:url,fetchUrl:absUrl,type:"html"};}',
    '}',
    'if(pdfMatch){return{originalUrl:url,fetchUrl:url,type:"pdf"};}',
    'return{originalUrl:url,fetchUrl:url,type:"html"};',
    '});',
    // Parallel tab creation (Promise.all — faster than sequential)
    'const tabs=await Promise.all(processed.map(async function(p){',
    'const tab=await session.createTarget({url:"about:blank",background:true});',
    'return{targetId:tab.targetId,originalUrl:p.originalUrl,fetchUrl:p.fetchUrl,type:p.type,sessionId:tab.sessionId};',
    '}));',
    // Sequential navigation (must be sequential due to session.use() — Trap 2)
    'for(const tab of tabs){',
    'await session.use(tab.targetId);',
    'await session.Page.enable();',
    'await session.Page.setLifecycleEventsEnabled({enabled:true});',
    'await session.Page.navigate({url:tab.fetchUrl});',
    '}',
    // Sequential extraction with per-tab error isolation
    'const results=[];',
    'for(const tab of tabs){',
    extractTabCode,
    '}',
    // Parallel tab close (Promise.allSettled — don't let one failure block others)
    'await Promise.allSettled(tabs.map(function(tab){return session.closeTab(tab.targetId,tab.sessionId).catch(function(){});}));',
    'return JSON.stringify(results);',
  ].join('');
}

/**
 * searchCode: Pure — returns full JS for single search.
 * Uses Runtime.evaluate for Google result extraction with count inlined
 * into the expression string.
 */
export function searchCode(
  query: string,
  count: number,
  port: number,
): string {
  const conn = connectCode(port);
  const queryJson = JSON.stringify(query);

  return [
    conn,
    'const count=' + count + ';',
    'const {targetId, sessionId}=await session.createTarget({url:"about:blank",background:true});',
    'try{',
    'await session.use(targetId);',
    'await session.Page.enable();',
    'await session.Page.setLifecycleEventsEnabled({enabled:true});',
    'const ready=session.waitFor("Page.lifecycleEvent",function(p){return p.name==="networkIdle";},30000);',
    'await session.Page.navigate({url:"https://www.google.com/search?q="+encodeURIComponent(' +
      queryJson +
      ')+"&num="+count+"&hl=en"});',
    'await ready;',
    // Extract Google search results via Runtime.evaluate — count inlined
    'var _searchResult;',
    'try{',
    'const r=await session.Runtime.evaluate({',
    "  expression:'JSON.stringify([...document.querySelectorAll(\"a[href]\")].filter(a => {try{var u=new URL(a.href);return u.hostname!==location.hostname&&u.pathname!==\"/search\"}catch(e){return false}}).slice(0,' + count + ').map(el=>({url:el.href||\"\",title:(el.querySelector(\"h3\")?.textContent||el.textContent||\"\").trim(),snippet:((el.closest(\"[data-hveid]\")?.textContent||\"\").split(/" + GOOGLE_TRANSLATE_PATTERN.source + "/).pop()||\"\").trim().slice(0,200)})).filter(r => r.title&&r.url))',",
    '  returnByValue:true',
    '});',
    '_searchResult=r.result.value;',
    '}catch(e){_searchResult=JSON.stringify({_error:"extraction_failed: "+e.message})}',
    'return _searchResult;',
    '}finally{',
    'session.closeTab(targetId,sessionId).catch(function(){});',
    '}',
  ].join('');
}

/**
 * batchSearchCode: Pure — returns full JS for batch search.
 * Queries Google in parallel tabs, deduplicates by URL, sorts by snippet length.
 * Uses callFunctionOn for faster DOM extraction.
 */
export function batchSearchCode(
  queries: string[],
  count: number,
  port: number,
): string {
  const conn = connectCode(port);
  const queriesJson = JSON.stringify(queries);

  return [
    conn,
    'const queries=' + queriesJson + ';',
    'const count=' + count + ';',
    // Parallel tab creation
    'const tabs=await Promise.all(queries.map(async function(q){',
    'const tab=await session.createTarget({url:"about:blank",background:true});',
    'return{targetId:tab.targetId,query:q,sessionId:tab.sessionId};',
    '}));',
    // Sequential navigation
    'for(const tab of tabs){',
    'await session.use(tab.targetId);',
    'await session.Page.enable();',
    'await session.Page.setLifecycleEventsEnabled({enabled:true});',
'await session.Page.navigate({url:"https://www.google.com/search?q="+encodeURIComponent(tab.query)+"&num="+count+"&hl=en"});',
'}',

    // Sequential extraction
    'const allResults=[];',
    'for(const tab of tabs){',
    'await session.use(tab.targetId);',
    // Poll readyState via evaluate
    'for(let i=0;i<100;i++){',
    'try{',
    'const r=await session.Runtime.evaluate({',
    '  expression:"document.readyState===\'complete\'",',
    '  returnByValue:true',
    '});',
    'if(r.result&&r.result.value)break;',
    '}catch(e){}',
    'await new Promise(r=>setTimeout(r,200));',
    '}',
    // Extract Google search results via evaluate
    'let parsed=[];',
    'try{',
    'const r=await session.Runtime.evaluate({',
    "  expression:'JSON.stringify([...document.querySelectorAll(\"a[href]\")].filter(a => {try{var u=new URL(a.href);return u.hostname!==location.hostname&&u.pathname!==\"/search\"}catch(e){return false}}).slice(0,' + count + ').map(el=>({url:el.href||\"\",title:(el.querySelector(\"h3\")?.textContent||el.textContent||\"\").trim(),snippet:((el.closest(\"[data-hveid]\")?.textContent||\"\").split(/" + GOOGLE_TRANSLATE_PATTERN.source + "/).pop()||\"\").trim().slice(0,200)})).filter(r => r.title&&r.url))',",
    'returnByValue:true',
    '});',
    'parsed=JSON.parse(r.result.value);',
    '}catch(e){}',
    'for(const item of parsed){item._query=tab.query;allResults.push(item);}',
    '}',
    // Parallel tab close
    'await Promise.allSettled(tabs.map(function(tab){return session.closeTab(tab.targetId,tab.sessionId).catch(function(){});}));',
    'const seen=new Set();',
    'const deduped=allResults.filter(function(r){if(seen.has(r.url))return false;seen.add(r.url);return true;});',
    'deduped.sort(function(a,b){return(b.snippet?.length||0)-(a.snippet?.length||0);});',
    'return JSON.stringify({results:deduped,meta:{queries:queries,total_unique:deduped.length,total_raw:allResults.length}});',
  ].join('');
}

/**
 * batchHarvestCode: Pure — returns full JS for batch harvest pipeline.
 * Phase 1: parallel search all queries → dedup → rank → pick top M.
 * Phase 2: parallel follow top M URLs.
 * All phases use callFunctionOn for faster extraction and Promise.allSettled
 * for error-isolated batch parallelism.
 */
export function batchHarvestCode(
  queries: string[],
  count: number,
  maxPages: number,
  timeoutMs: number,
  port: number,
): string {
  const conn = connectCode(port);
  const queriesJson = JSON.stringify(queries);
  const selJson = JSON.stringify('article, main, [role=main]');
  const extractFn = extractionCode();

  const extractTabCode = [
    'await session.use(tab.targetId);',
    'let content="";let loadError=null;let isJunk=false;',
    'try{await Promise.race([',
    '(async()=>{',
    'if(tab.type==="pdf"){',
    'await new Promise(r=>setTimeout(r,3000));',
    'try{const r=await session.Runtime.evaluate({',
    '  expression:"document.body?.innerText||document.querySelector(\\"embed\\")?.shadowRoot?.textContent||\\"\\"",',
    '  returnByValue:true});',
    'content=(r.result&&r.result.value)||"";',
    '}catch(e){loadError="pdf_extraction_failed: "+e.message;}',
    '}else{',
    // Poll readyState before extraction (Trap 2: polling > waitFor in multi-tab)
    readyStatePoll(),
    'if(!loadError){',
    'const r=await session.Runtime.evaluate({',
    "  expression:'document.querySelector(\"'+selector+'\")?.innerText||document.body?.innerText||\"\"',",
    'returnByValue:true});',
    'content=(r.result&&r.result.value)||"";',
    '}',
    '}',
    qualityGateCode(),
    '})(),',
    'new Promise((_,reject)=>setTimeout(()=>reject(new Error("tab_timeout")),timeoutMs))',
    ']);}catch(e){loadError=e.message;}',
    'pages.push({url:tab.originalUrl||tab.url,title:tab.title,query:tab.query,content:isJunk?"":content,...(loadError?{_error:loadError}:{})});',
  ].join('');

  return [
    conn,
    'const queries=' + queriesJson + ';',
    'const count=' + count + ';',
    'const maxPages=' + maxPages + ';',
    'const timeoutMs=' + timeoutMs + ';',
    'const selector=' + selJson + ';',

    // ── Phase 1: Parallel Search ──

    // Parallel tab creation for search
    'const searchTabs=await Promise.all(queries.map(async function(q){',
    'const tab=await session.createTarget({url:"about:blank",background:true});',
    'return{targetId:tab.targetId,query:q,sessionId:tab.sessionId};',
    '}));',
    // Sequential navigation for search tabs
    'for(const tab of searchTabs){',
    'await session.use(tab.targetId);',
    'await session.Page.enable();',
    'await session.Page.setLifecycleEventsEnabled({enabled:true});',
    'await session.Page.navigate({url:"https://www.google.com/search?q="+encodeURIComponent(tab.query)+"&num="+count+"&hl=en"});',
    '}',
    // Sequential extraction
    'const allSearchResults=[];',
    'for(const tab of searchTabs){',
    'await session.use(tab.targetId);',
    'for(let i=0;i<100;i++){',
    'try{',
    'const r=await session.Runtime.evaluate({',
    '  expression:"document.readyState===\'complete\'",',
    '  returnByValue:true',
    '});',
    'if(r.result&&r.result.value)break;',
    '}catch(e){}',
    'await new Promise(r=>setTimeout(r,200));',
    '}',
    'let parsed=[];',
    'try{',
    'const r=await session.Runtime.evaluate({',
    "  expression:'JSON.stringify([...document.querySelectorAll(\"a[href]\")].filter(a => {try{var u=new URL(a.href);return u.hostname!==location.hostname&&u.pathname!==\"/search\"}catch(e){return false}}).slice(0,' + count + ').map(el=>({url:el.href||\"\",title:(el.querySelector(\"h3\")?.textContent||el.textContent||\"\").trim(),snippet:((el.closest(\"[data-hveid]\")?.textContent||\"\").split(/" + GOOGLE_TRANSLATE_PATTERN.source + "/).pop()||\"\").trim().slice(0,200)})).filter(r => r.title&&r.url))',",
    'returnByValue:true',
    '});',
    'parsed=JSON.parse(r.result.value);',
    '}catch(e){}',
    'for(const item of parsed){item._query=tab.query;allSearchResults.push(item);}',
    '}',
    // Parallel tab close for search tabs
    'await Promise.allSettled(searchTabs.map(function(tab){return session.closeTab(tab.targetId,tab.sessionId).catch(function(){});}));',

    // Dedup + rank + pick top M
    'const seen=new Set();',
    'const deduped=allSearchResults.filter(function(r){if(seen.has(r.url))return false;seen.add(r.url);return true;});',
    'deduped.sort(function(a,b){return(b.snippet?.length||0)-(a.snippet?.length||0);});',
    'const topUrls=deduped.slice(0,maxPages);',

    // ── Phase 2: Parallel Follow ──

    'const followItems=[];',
    'for(const item of topUrls){',
    'const pdfMatch=item.url.match(/(\\/pdf\\/|\\.pdf$)/i);',
    'if(pdfMatch&&pdfMatch[1]==="/pdf/"){',
    'const absUrl=item.url.replace(/\\/pdf\\/(.+)/,"/abs/$1").replace(/\\.pdf$/,"");',
    'followItems.push({url:absUrl,originalUrl:item.url,title:item.title,query:item._query,type:"html"});',
    '}else if(pdfMatch){',
    'followItems.push({url:item.url,originalUrl:item.url,title:item.title,query:item._query,type:"pdf"});',
    '}else{',
    'followItems.push({url:item.url,originalUrl:item.url,title:item.title,query:item._query,type:"html"});',
    '}',
    '}',
    // Parallel tab creation for follow
    'const followTabs=await Promise.all(followItems.map(async function(item){',
    'const tab=await session.createTarget({url:"about:blank",background:true});',
    'return{targetId:tab.targetId,url:item.url,originalUrl:item.originalUrl,title:item.title,query:item.query,type:item.type,sessionId:tab.sessionId};',
    '}));',
    // Sequential navigation for follow tabs
    'for(const tab of followTabs){',
    'await session.use(tab.targetId);',
    'await session.Page.enable();',
    'await session.Page.setLifecycleEventsEnabled({enabled:true});',
    'await session.Page.navigate({url:tab.url});',
    '}',
    // Sequential extraction with per-tab error isolation
    'const pages=[];',
    'for(const tab of followTabs){',
    extractTabCode,
    '}',
    // Parallel tab close for follow tabs
    'await Promise.allSettled(followTabs.map(function(tab){return session.closeTab(tab.targetId,tab.sessionId).catch(function(){});}));',

    // Return result
    'return JSON.stringify({',
    'search_results:deduped,',
    'read_pages:pages,',
    'meta:{',
    'queries:queries,',
    'total_search_results:allSearchResults.length,',
    'unique_urls:deduped.length,',
    'pages_read:pages.filter(function(p){return !p._error&&p.content.length>0;}).length,',
    'pages_skipped:pages.filter(function(p){return p._error;}).length',
    '}',
    '});',
  ].join('');
}

/**
 * sessionHealthCheckCode: Pure — returns JS that closes stale tabs
 * (non-about:blank, non-chrome://) before starting a batch operation.
 */
export function sessionHealthCheckCode(port: number): string {
  return `return (async function(){
    if(!session.isConnected()){try{await session.connect({port:${port}})}catch(e){}}
    if(session.isConnected()){
      for(var targetId of session.agentTabs.keys()){
        try{await session.closeTab(targetId);}catch(e){}
      }
    }
    return "ok";
  })()`;
}
