# AST Discovery — Structured Knowledge Trees for Web Research

When an agent uses gsearch to perform multi-page research, the raw approach — dumping page text into context — wastes tokens, loses provenance, and breeds hallucination. AST Discovery solves this by extracting structured content into a tree/graph format that serves as the agent's canonical knowledge base.

## Concept

When an agent visits web pages during discovery, it extracts structured content — headings, sections, entities, claims — into a tree or graph. This tree:

- **Tracks provenance** — each node links to its source URL and extraction timestamp.
- **Organizes content hierarchically** — page → section → claim → entity.
- **Enables cross-referencing** — nodes from different sources merge on shared topics.
- **Serves as the canonical knowledge base** — the agent reasons from the tree, not from raw text.

The tree is the single source of truth for the session. Everything the agent knows is in it. Everything the agent says must trace back to a node in it.

## Benefits

### 1. Traceability

The agent can explain exactly where each piece of information came from. Every node stores its source URL, extraction time, and (optionally) a CSS selector. When you ask "where did you find that?", the answer is already attached to the claim node.

### 2. Reference-ready citations

Citation isn't bolted on after generation — it's built into the data model. Every claim node requires a `source` field. The agent cannot create a claim without immediately tying it to a URL and section.

### 3. Token efficiency

A structured tree uses roughly 60% fewer tokens than equivalent raw text:

- Headings replace descriptive prose (e.g., "Section on machine learning in finance" → node with `type:"section"` and `content:"Machine Learning in Finance"`).
- Deduplication is built in — if two pages say the same thing, they share one node instead of repeating the text.
- Tree structure replaces connective prose — the hierarchy is explicit, not narrated.

### 4. Hallucination prevention

Every claim node has a required `source` field. The agent cannot output a claim that lacks a source URL. Before generating a response, the agent walks the tree to find supporting nodes; claims without matchings nodes are suppressed. This provides a **closed-world guarantee**: if it's not in the tree, the agent doesn't say it.

### 5. Cross-source merging

When two pages mention the same entity or fact, the merge step collapses them into one node with multiple sources. This naturally builds consensus views — the more sources a node has, the more confidently the agent can assert it. Contradictory claims are preserved as separate nodes with an explicit "contradicts" edge.

## Tree Structure

### Data model

```
DiscoverySession {
  query: string,
  timestamp: string,
  nodes: DiscoveryNode[],
  edges: { from: nodeId, to: nodeId, relation: string }[]
}

DiscoveryNode {
  id: string,
  type: "page" | "section" | "claim" | "entity" | "quote",
  source: { url: string, extractedAt: string, selector?: string },
  content: string,
  confidence: number,        // 0–1 based on source reliability
  children: string[]          // child node IDs
}
```

### Node types

| Type | Content field holds | Parent | Children |
|------|---------------------|--------|----------|
| `page` | Page title or URL | none (root) | section IDs |
| `section` | Heading text (h1–h6) | page ID | claim IDs |
| `claim` | Key statement or fact | section ID | entity IDs |
| `entity` | Named entity (person, org, concept) | claim ID | — |
| `quote` | Verbatim quote with attribution | section ID or claim ID | — |

### Edges

Edges capture relationships between nodes that the hierarchy alone doesn't express:

| Relation | Meaning |
|----------|---------|
| `mentions` | Node A references the entity or topic in node B |
| `contradicts` | Node A makes a claim that contradicts node B |
| `supports` | Node A provides evidence for node B |
| `extends` | Node A elaborates on the topic introduced by node B |
| `see_also` | General cross-reference |

## How It's Built

The AST is constructed in a six-step pipeline that runs after each page is harvested.

### Step 1: Page node

When gsearch follows a URL, it creates a `page` node with `source.url` set to the fetched URL and `extractedAt` set to the current timestamp. Every subsequent node from that page inherits this URL.

### Step 2: Section nodes

The agent extracts `h1`–`h6` headings from the page content. Each heading becomes a `section` node, parented to the page node. The heading level determines nesting: `h2` nodes are children of the nearest `h1` node, and so on.

```json
{
  "id": "s1",
  "type": "section",
  "source": { "url": "https://example.com/article", "extractedAt": "2026-07-19T08:00:00Z" },
  "content": "Transformer Architectures for Time Series",
  "children": ["c1", "c2"]
}
```

### Step 3: Claim nodes

Under each section, the agent extracts key statements — sentences or short paragraphs that convey factual information. Claims are the atomic unit of knowledge in the tree. Each claim node must have a non-empty `content` field and a valid `source`.

```json
{
  "id": "c1",
  "type": "claim",
  "source": { "url": "https://example.com/article", "extractedAt": "2026-07-19T08:00:00Z" },
  "content": "Informer model reduces time-series MSE by 38% over Transformer baseline",
  "confidence": 0.85,
  "children": ["e1"]
}
```

### Step 4: Entity nodes

Named entities — people, organizations, concepts, technologies — are extracted from claims and promoted to their own nodes. An entity node stores the canonical name and is linked back to every claim that mentions it via edges. This enables the agent to answer questions like "what do you know about LSTMs?" by gathering all claims with edges to the LSTM entity node.

### Step 5: Merge

After all pages in a batch are processed, the merge phase runs:

- **Same URL visited twice** — existing node is reused, timestamp updated.
- **Different URLs, same heading text** — section nodes are merged, both sources are added to the node.
- **Different URLs, same claim** — claim nodes are merged, the source list grows.
- **Contradictory claims** — both nodes are kept with `confidence` adjusted downward. An edge with relation `"contradicts"` is added between them.
- **New entity found** — an entity node is created and edges are added from all claims that reference it.

### Step 6: Edge creation

Edges are created last, after all nodes are in place. The agent scans claims for cross-references — when claim A mentions an entity that was introduced in claim B, an edge `A → B` with relation `"references"` is created. Similarly, explicit contradictions, support, or extension relationships are detected and recorded.

## Integration with gsearch

The AST is built after content extraction as part of the batch harvest pipeline.

```
gsearch batch harvest "query1" "query2"
  │
  ├─ Phase 1: Search → URLs (gsearch batch search)
  │
  ├─ Phase 2: Follow → pages + AST extraction
  │     │
  │     ├─ content: "..." (raw text, as today)
  │     └─ ast: DiscoveryNode[] (new structured output)
  │
  └─ Phase 3: Merge → consolidated tree
        ├─ nodes merged by topic
        ├─ edges for cross-references
        └─ meta (queries used, coverage, node count)
```

Phase 2 runs in parallel across all URLs, one CDP tab per page. Each tab independently extracts its content and its AST. Phase 3 runs serially — it needs the full set of nodes before it can merge and build edges.

The raw `content` field is preserved so that the agent can fall back to full text when the AST misses nuance. But for most reasoning tasks, the agent operates on the tree.

## Output Format (when requested)

When the agent generates output with AST provenance, the structure follows this schema:

```json
{
  "_ast": {
    "query": "latest ML finance papers",
    "nodes": [
      {
        "id": "p1",
        "type": "page",
        "source": { "url": "https://arxiv.org/list/q-fin/2025-12", "extractedAt": "2026-07-19T08:00:00Z" },
        "content": "Quantitative Finance December 2025",
        "children": ["s1", "s2"]
      },
      {
        "id": "s1",
        "type": "section",
        "source": { "url": "https://arxiv.org/list/q-fin/2025-12", "extractedAt": "2026-07-19T08:00:00Z" },
        "content": "Machine Learning in Finance",
        "children": ["c1", "c2"]
      },
      {
        "id": "c1",
        "type": "claim",
        "source": { "url": "https://arxiv.org/list/q-fin/2025-12", "extractedAt": "2026-07-19T08:00:00Z" },
        "content": "LSTM-Random Forest hybrid achieves 78% accuracy in stock prediction",
        "confidence": 0.85,
        "children": ["e1"]
      },
      {
        "id": "c2",
        "type": "claim",
        "source": { "url": "https://arxiv.org/list/q-fin/2025-12", "extractedAt": "2026-07-19T08:00:00Z" },
        "content": "Attention-based models outperform RNNs on FX forecasting by 12%",
        "confidence": 0.82,
        "children": []
      },
      {
        "id": "e1",
        "type": "entity",
        "source": { "url": "https://arxiv.org/list/q-fin/2025-12", "extractedAt": "2026-07-19T08:00:00Z" },
        "content": "LSTM-Random Forest",
        "children": []
      }
    ],
    "edges": [
      { "from": "c1", "to": "e1", "relation": "mentions" },
      { "from": "c2", "to": "c1", "relation": "see_also" }
    ],
    "meta": {
      "queries_used": ["latest ML finance papers"],
      "pages_harvested": 1,
      "nodes_total": 5,
      "coverage": "single page"
    }
  }
}
```

The `_ast` key is used only when the user or downstream system explicitly requests structured output. Normal conversational output still reads as natural prose; the tree works silently in the background as the agent's knowledge source.

## Anti-Hallucination Property

The AST provides a **closed-world guarantee**: the agent can only reference claims that exist as nodes in the tree. Any output without a matching source node is flagged as ungrounded.

### Enforcement

Before the agent generates a response, it walks the tree to find supporting nodes for each claim it intends to make:

1. For each claim in the planned response, search the tree for a node with matching `content`.
2. If found → the claim is grounded. Include the source URL in the citation.
3. If not found → the claim is suppressed. The agent must either find it (re-scan or re-query) or admit it cannot find the claim.

### What this prevents

- **False attribution** — the agent cannot invent a fact and attach a plausible URL. The fact must exist in the tree, which was built from actual pages.
- **Conflation** — the agent cannot merge two distinct claims from different sources. Each node is discrete and source-tagged.
- **Guessing** — when a question is unanswerable from the tree, the agent must say so rather than fabricate.

### Limitations

The closed-world guarantee is only as good as the tree's construction. If the extraction step misses a claim (e.g., the text was in a JavaScript-rendered element the selector didn't catch), the tree is incomplete. The agent should note low coverage in its `meta.coverage` field when it detects sparsely populated sections.

## Merge Strategy

When `batch harvest` reads multiple pages, the merge step applies these rules:

| Condition | Action |
|-----------|--------|
| Same URL visited twice | Reuse existing node, update `extractedAt` timestamp |
| Different URLs, same heading text | Merge section nodes, append the second URL to `source` (becomes multi-source) |
| Different URLs, identical claim | Merge claim nodes, source list grows — `confidence` increases |
| Similar claim (~80% semantic similarity) | Merge claim nodes, keep the longer `content`, average `confidence` |
| Contradictory claims | Keep both nodes as separate claims, reduce both `confidence` by 0.2, add edge with relation `"contradicts"` |
| New entity discovered | Create entity node, add `mentions` edges from all claims that reference it |
| Same entity, different names | Normalize to canonical name (longest form wins), store aliases in a separate `aliases` field on the entity node |

### Confidence scoring

Confidence is computed from three factors that are combined multiplicatively:

- **Source authority**: known journals and institutions = 1.0, established news outlets = 0.9, personal blogs = 0.5, forums = 0.3.
- **Freshness**: content less than 1 year old = 1.0, 1–3 years = 0.8, 3–5 years = 0.6, over 5 years = 0.4.
- **Cross-source confirmation**: 3+ independent sources agree = 1.2×, 2 sources agree = 1.0×, single source = 0.8×.

Final confidence is clamped to `[0, 1]`.

## When to Use / When Not To

| Use AST | Don't use AST |
|---------|---------------|
| Multi-page research sessions (>2 pages) | Single page, single fact query |
| Comparing sources on the same topic | Raw data extraction (CSV, tables, API responses) |
| Building a literature review | Quick factual lookups (one answer, one source) |
| User needs citations and provenance | User only needs the answer (no citation required) |
| Long discovery session (>5 pages) | Short 1–2 page session |
| Synthesizing across multiple authors | Copying verbatim from a single source |

The AST has overhead — extracting sections, merging nodes, computing confidence — that isn't justified for a single factoid lookup. Use the raw `content` path for those cases. For any session where you'll cite two or more sources, the AST pays for itself in citation quality and hallucination reduction.

## Implementation Notes

### Where the tree lives

The AST is built server-side (in the gsearch batch harvest JS) or client-side (in the AI agent's context depending on token budget). For heavy research sessions, building it server-side is more efficient because the extraction runs once and the tree is sent as a compact structure.

### Token budget pruning

When the tree must fit in a limited context window, apply these pruning rules in order:

1. **Remove nodes with `confidence < 0.3`** — unreliable sources are not worth the tokens.
2. **Deduplicate near-identical claims** — if two claim nodes have >90% character overlap, keep the one with higher `confidence`.
3. **Truncate `content` fields** — cap at 500 characters per node. Longer content is ellipsized.
4. **Collapse empty branches** — sections with no child claims are removed (their content is preserved as a single claim node instead).
5. **Strip `source.selector`** — the CSS selector is useful for debugging but rarely needed in the final tree.

### Extraction heuristics

- **Claim boundaries**: the agent splits section content on sentence boundaries, then filters for sentences containing factual assertions (verbs like "achieves", "reduces", "increases", "is", "was found"). Sentences that are purely transitional ("In this section we discuss...") or referential ("As shown in Figure 3...") are skipped.
- **Entity recognition**: named entities are identified by noun phrases that match known entity patterns — capitalized terms, quoted terms, technical terms followed by parenthetical abbreviations (e.g., "Long Short-Term Memory (LSTM)").
- **Quote detection**: text surrounded by quotation marks or rendered as `<blockquote>` is extracted as a `quote` node rather than a `claim` node, preserving the verbatim text.

### Error handling

- **Empty page**: if a page returns no section headings and no claimable content, a page node is still created with an empty `children` array. This prevents hallucination about a page the agent never actually read.
- **Extraction timeout**: if a page takes longer than 10 seconds to extract structure, the agent falls back to using the raw `content` text as a single claim node.
- **Garbled text**: nodes with `content` that fails a basic language model perplexity check are tagged `confidence: 0.1` and removed during pruning.

### Reference from other files

The AST pipeline hooks into the batch harvest system described in [`commands.md`](commands.md). The `--ast` flag (planned, not yet implemented) enables AST extraction alongside content extraction. See the gsearch skill playbook for how to request AST-structured responses.
