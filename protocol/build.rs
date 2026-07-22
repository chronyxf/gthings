use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::path::Path;

// ---------------------------------------------------------------------------
// Data structures matching CDP protocol JSON
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Protocol {
    #[allow(dead_code)]
    version: Option<serde_json::Value>,
    domains: Vec<Domain>,
}

#[derive(Debug, Deserialize)]
struct Domain {
    domain: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    types: Vec<TypeDef>,
    #[serde(default)]
    commands: Vec<CommandDef>,
    #[serde(default)]
    events: Vec<EventDef>,
    #[serde(default)]
    #[allow(dead_code)]
    dependencies: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TypeDef {
    id: String,
    #[serde(rename = "type")]
    type_: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    properties: Vec<RawProperty>,
    #[serde(default, rename = "enum")]
    enum_values: Vec<String>,
    #[serde(default)]
    items: Option<Box<ItemsDef>>,
    #[serde(default)]
    #[allow(dead_code)]
    experimental: bool,
    #[serde(default)]
    #[allow(dead_code)]
    deprecated: bool,
}

#[derive(Debug, Deserialize)]
struct CommandDef {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    experimental: bool,
    #[serde(default)]
    #[allow(dead_code)]
    deprecated: bool,
    #[serde(default)]
    redirect: Option<String>,
    #[serde(default)]
    parameters: Vec<RawProperty>,
    #[serde(default)]
    returns: Vec<RawProperty>,
}

#[derive(Debug, Deserialize)]
struct EventDef {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    experimental: bool,
    #[serde(default)]
    #[allow(dead_code)]
    deprecated: bool,
    #[serde(default)]
    parameters: Vec<RawProperty>,
}

#[derive(Debug, Deserialize)]
struct RawProperty {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    optional: bool,
    #[serde(default)]
    #[allow(dead_code)]
    experimental: bool,
    #[serde(default)]
    #[allow(dead_code)]
    deprecated: bool,
    #[serde(rename = "type", default)]
    prop_type: Option<String>,
    #[serde(default)]
    items: Option<Box<ItemsDef>>,
    #[serde(default, rename = "enum")]
    #[allow(dead_code)]
    enum_values: Vec<String>,
    #[serde(rename = "$ref", default)]
    ref_: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ItemsDef {
    #[serde(rename = "type", default)]
    type_: Option<String>,
    #[serde(rename = "$ref", default)]
    ref_: Option<String>,
    #[serde(default)]
    items: Option<Box<ItemsDef>>,
}

// ---------------------------------------------------------------------------
// Naming utilities
// ---------------------------------------------------------------------------

fn domain_to_module(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                s.push('_');
            }
            for lower in c.to_lowercase() {
                s.push(lower);
            }
        } else {
            s.push(c);
        }
    }
    s
}

fn camel_to_snake(name: &str) -> String {
    let mut result = String::with_capacity(name.len() + 4);
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                let prev_is_lower = chars[i - 1].is_lowercase();
                let next_is_lower = i + 1 < chars.len() && chars[i + 1].is_lowercase();
                if prev_is_lower || next_is_lower {
                    result.push('_');
                }
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "abstract"
            | "as"
            | "become"
            | "box"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "do"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "final"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "macro"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "override"
            | "priv"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "typeof"
            | "unsafe"
            | "use"
            | "virtual"
            | "where"
            | "while"
            | "yield"
            | "try"
            | "async"
            | "await"
            | "dyn"
            | "gen"
    )
}

fn safe_ident(name: &str) -> String {
    if is_rust_keyword(name) {
        format!("{}_", name)
    } else if name.is_empty() {
        "_empty".to_string()
    } else {
        name.to_string()
    }
}

fn enum_variant_name(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut upper_next = true;
    for c in value.chars() {
        if c == '-' || c == '_' || c == ' ' {
            upper_next = true;
        } else if upper_next {
            result.push(c.to_ascii_uppercase());
            upper_next = false;
        } else {
            result.push(c);
        }
    }
    if result.starts_with(|c: char| c.is_ascii_digit()) {
        result.insert(0, '_');
    }
    safe_ident(&result)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

// ---------------------------------------------------------------------------
// Type resolution helpers
// ---------------------------------------------------------------------------

/// Resolve a `$ref` to its Rust type path. Returns the raw type name for same-domain,
/// or `{mod_name}::TypeName` path for cross-domain (WITHOUT super:: prefix, since
/// all generated code sits at crate root level).
fn resolve_ref(ref_name: &str, current_domain: &str) -> String {
    if let Some(dot) = ref_name.find('.') {
        let domain_part = &ref_name[..dot];
        let type_part = &ref_name[dot + 1..];
        if domain_part == current_domain {
            type_part.to_string()
        } else {
            let mod_name = domain_to_module(domain_part);
            format!("super::{}::{}", mod_name, type_part)
        }
    } else {
        ref_name.to_string()
    }
}

/// Map a raw CDP type to Rust type string.
fn resolve_raw_type(
    type_name: &str,
    items: &Option<Box<ItemsDef>>,
    _enum_values: &[String],
) -> String {
    match type_name {
        "string" => "String".to_string(),
        "boolean" => "bool".to_string(),
        "integer" => "i64".to_string(),
        "number" => "f64".to_string(),
        "any" => "serde_json::Value".to_string(),
        "binary" => "String".to_string(),
        "array" => {
            if let Some(items) = items {
                if let Some(ref_) = &items.ref_ {
                    format!("Vec<{}>", ref_)
                } else if let Some(inner_type) = &items.type_ {
                    let inner_str = resolve_raw_type(inner_type, &items.items, &[]);
                    format!("Vec<{}>", inner_str)
                } else {
                    "Vec<serde_json::Value>".to_string()
                }
            } else {
                "Vec<serde_json::Value>".to_string()
            }
        }
        "object" => "serde_json::Value".to_string(),
        _ => type_name.to_string(),
    }
}

/// Fully resolve a property's Rust type, handling $ref, arrays, etc.
/// `current_type_id` is used for recursive-type detection (wraps in Box).
fn resolve_property_type(
    prop: &RawProperty,
    current_domain: &str,
    current_type_id: Option<&str>,
) -> String {
    let base = resolve_property_type_inner(prop, current_domain);
    // Check for recursive self-reference: if the property is optional and
    // the resolved type (without Option wrapper) matches the current type,
    // wrap in Box to break the cycle.
    if prop.optional {
        if let Some(ctid) = current_type_id {
            // The base type is "Option<T>" — check if T matches ctid
            // But base is just "T" (the Option is added in generate_property_field)
            // So we check if base == ctid or if it's a Vec/Box situation
            if base == ctid {
                return format!("Box<{}>", base);
            }
        }
    }
    base
}

fn resolve_property_type_inner(prop: &RawProperty, current_domain: &str) -> String {
    // If it has a $ref, resolve it
    if let Some(ref_name) = &prop.ref_ {
        return resolve_ref(ref_name, current_domain);
    }

    // If it has a type, map it
    if let Some(type_name) = &prop.prop_type {
        match type_name.as_str() {
            "string" => "String".to_string(),
            "boolean" => "bool".to_string(),
            "integer" => "i64".to_string(),
            "number" => "f64".to_string(),
            "any" => "serde_json::Value".to_string(),
            "binary" => "String".to_string(),
            "array" => {
                if let Some(items) = &prop.items {
                    if let Some(ref_) = &items.ref_ {
                        let resolved = resolve_ref(ref_, current_domain);
                        format!("Vec<{}>", resolved)
                    } else if let Some(inner_type) = &items.type_ {
                        let inner_str = resolve_raw_type(inner_type, &items.items, &[]);
                        format!("Vec<{}>", inner_str)
                    } else {
                        "Vec<serde_json::Value>".to_string()
                    }
                } else {
                    "Vec<serde_json::Value>".to_string()
                }
            }
            "object" => "serde_json::Value".to_string(),
            _ => "serde_json::Value".to_string(),
        }
    } else {
        "serde_json::Value".to_string()
    }
}

fn fix_array_refs(ty: &str, current_domain: &str) -> String {
    if ty.starts_with("Vec<") && ty.ends_with('>') {
        let inner = &ty[4..ty.len() - 1];
        if inner.contains('.') {
            let resolved = resolve_ref(inner, current_domain);
            return format!("Vec<{}>", resolved);
        }
    }
    ty.to_string()
}

// ---------------------------------------------------------------------------
// Code generator
// ---------------------------------------------------------------------------

struct Generator {
    output: String,
    indent: usize,
}

impl Generator {
    fn new() -> Self {
        Self {
            output: String::new(),
            indent: 0,
        }
    }

    fn line(&mut self, s: impl AsRef<str>) {
        for _ in 0..self.indent {
            self.output.push_str("    ");
        }
        self.output.push_str(s.as_ref());
        self.output.push('\n');
    }

    fn blank(&mut self) {
        self.output.push('\n');
    }

    fn open_block(&mut self, s: impl AsRef<str>) {
        self.line(s);
        self.indent += 1;
    }

    fn close_block(&mut self, s: impl AsRef<str>) {
        self.indent -= 1;
        self.line(s);
    }

    fn result(self) -> String {
        self.output
    }
}

// ---------------------------------------------------------------------------
// Code generation helpers
// ---------------------------------------------------------------------------

fn generate_type_def(g: &mut Generator, type_def: &TypeDef, current_domain: &str) {
    match type_def.type_.as_str() {
        "object" if !type_def.properties.is_empty() => {
            if let Some(desc) = &type_def.description {
                for line in desc.lines() {
                    g.line(format!("/// {}", line));
                }
            }
            g.line("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]");
            g.open_block(format!("pub struct {} {{", type_def.id));
            for p in &type_def.properties {
                generate_property_field(g, p, current_domain, Some(&type_def.id));
            }
            g.close_block("}");
        }
        "string" | "boolean" | "integer" | "number" => {
            if !type_def.enum_values.is_empty() {
                if let Some(desc) = &type_def.description {
                    for line in desc.lines() {
                        g.line(format!("/// {}", line));
                    }
                }
                g.line("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]");
                g.open_block(format!("pub enum {} {{", type_def.id));
                for val in &type_def.enum_values {
                    let variant = enum_variant_name(val);
                    g.line(format!("#[serde(rename = {:?})]", val));
                    g.line(format!("{},", variant));
                }
                g.close_block("}");
            } else {
                let rust_ty =
                    resolve_raw_type(&type_def.type_, &type_def.items, &type_def.enum_values);
                if let Some(desc) = &type_def.description {
                    for line in desc.lines() {
                        g.line(format!("/// {}", line));
                    }
                }
                g.line(format!("pub type {} = {};", type_def.id, rust_ty));
            }
        }
        "array" => {
            let rust_ty = resolve_raw_type(&type_def.type_, &type_def.items, &[]);
            let rust_ty = fix_array_refs(&rust_ty, current_domain);
            if let Some(desc) = &type_def.description {
                for line in desc.lines() {
                    g.line(format!("/// {}", line));
                }
            }
            g.line(format!("pub type {} = {};", type_def.id, rust_ty));
        }
        "any" => {
            g.line(format!("pub type {} = serde_json::Value;", type_def.id));
        }
        "binary" => {
            g.line(format!("pub type {} = String;", type_def.id));
        }
        _ => {
            g.line(format!("pub type {} = serde_json::Value;", type_def.id));
        }
    }
}

fn generate_property_field(
    g: &mut Generator,
    prop: &RawProperty,
    current_domain: &str,
    current_type_id: Option<&str>,
) {
    let rust_field_name = safe_ident(&camel_to_snake(&prop.name));
    let rust_type = resolve_property_type(prop, current_domain, current_type_id);

    // Build serde attributes
    let mut attrs: Vec<String> = Vec::new();

    let json_name = &prop.name;
    let rust_name_stripped = rust_field_name.trim_end_matches('_');

    // Generate serde rename if JSON field name differs
    if json_name != rust_name_stripped || is_rust_keyword(rust_name_stripped) {
        attrs.push(format!("rename = {:?}", json_name));
    }

    if prop.optional {
        attrs.push("skip_serializing_if = \"Option::is_none\"".to_string());
    }

    let attrs_str = if attrs.is_empty() {
        String::new()
    } else {
        format!("#[serde({})]", attrs.join(", "))
    };

    if !attrs_str.is_empty() {
        g.line(&attrs_str);
    }

    if prop.optional {
        g.line(format!("pub {}: Option<{}>,", rust_field_name, rust_type));
    } else {
        g.line(format!("pub {}: {},", rust_field_name, rust_type));
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let manifest_dir_str = env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest_dir = Path::new(&manifest_dir_str);
    let browser_path = manifest_dir.join("browser_protocol.json");
    let js_path = manifest_dir.join("js_protocol.json");

    // Parse both protocol files
    let browser_data: Protocol =
        serde_json::from_str(&std::fs::read_to_string(&browser_path).unwrap())
            .expect("Failed to parse browser_protocol.json");
    let js_data: Protocol = serde_json::from_str(&std::fs::read_to_string(&js_path).unwrap())
        .expect("Failed to parse js_protocol.json");

    // Merge domains
    let mut all_domains: HashMap<String, Domain> = HashMap::new();
    for d in browser_data.domains {
        all_domains.insert(d.domain.clone(), d);
    }
    for d in js_data.domains {
        all_domains.insert(d.domain.clone(), d);
    }

    // Build sorted domain list
    let domain_names: Vec<String> = {
        let mut names: Vec<String> = all_domains.keys().cloned().collect();
        names.sort();
        names
    };

    // ---- Start generating ----
    let mut g = Generator::new();

    g.line("// AUTO-GENERATED by build.rs — do not edit by hand.");
    g.line("// Generated from browser_protocol.json + js_protocol.json");
    g.blank();

    // ---- Phase 1: Domain modules ----
    let mut all_commands: Vec<(String, String)> = Vec::new();
    let mut all_events: Vec<(String, String)> = Vec::new();

    for domain_name in &domain_names {
        let domain = &all_domains[domain_name];
        let mod_name = domain_to_module(domain_name);

        g.open_block(format!("pub mod {} {{", mod_name));
        g.blank();

        // Type definitions from domain's `types` array
        for type_def in &domain.types {
            generate_type_def(&mut g, type_def, domain_name);
            g.blank();
        }

        // Method name constants
        for cmd in &domain.commands {
            if cmd.redirect.is_some() {
                continue;
            }
            let const_name = cmd.name.to_uppercase();
            let method_str = format!("{}.{}", domain_name, cmd.name);
            g.line(format!(
                "pub const {}: &str = {:?};",
                const_name, method_str
            ));
        }
        g.blank();

        // Command Params & Return structs
        for cmd in &domain.commands {
            if cmd.redirect.is_some() {
                continue;
            }
            let cap_name = capitalize(&cmd.name);
            let params_type = format!("{}Params", cap_name);
            let return_type = format!("{}Return", cap_name);

            all_commands.push((domain_name.clone(), cmd.name.clone()));

            // Params struct
            if !cmd.parameters.is_empty() {
                g.line("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]");
                g.open_block(format!("pub struct {} {{", params_type));
                for p in &cmd.parameters {
                    generate_property_field(&mut g, p, domain_name, None);
                }
                g.close_block("}");
            } else {
                g.line("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]");
                g.line(format!("pub struct {};", params_type));
            }

            // Return struct
            if !cmd.returns.is_empty() {
                g.line("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]");
                g.open_block(format!("pub struct {} {{", return_type));
                for r in &cmd.returns {
                    generate_property_field(&mut g, r, domain_name, None);
                }
                g.close_block("}");
            } else {
                g.line("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]");
                g.line(format!("pub struct {};", return_type));
            }

            g.blank();
        }

        // Event param structs
        for evt in &domain.events {
            let evt_cap = capitalize(&evt.name);
            let params_type = format!("{}Params", evt_cap);
            all_events.push((domain_name.clone(), evt.name.clone()));

            if !evt.parameters.is_empty() {
                g.line("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]");
                g.open_block(format!("pub struct {} {{", params_type));
                for p in &evt.parameters {
                    generate_property_field(&mut g, p, domain_name, None);
                }
                g.close_block("}");
            } else {
                g.line("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]");
                g.line(format!("pub struct {};", params_type));
            }
            g.blank();
        }

        g.close_block("}");
        g.blank();
    }

    // ---- Phase 2: Command trait ----
    g.line("// ------------------------------------------------------------------");
    g.line("// Command trait");
    g.line("// ------------------------------------------------------------------");
    g.blank();
    g.line("pub trait Command: serde::Serialize {");
    g.line("    type Return: serde::de::DeserializeOwned;");
    g.line("    fn method(&self) -> &'static str;");
    g.line("}");
    g.blank();

    // ---- Phase 3: CdpMethod enum ----
    g.line("// ------------------------------------------------------------------");
    g.line("// CdpMethod enum");
    g.line("// ------------------------------------------------------------------");
    g.blank();
    g.line("#[derive(Debug, Clone)]");
    g.open_block("pub enum CdpMethod {");
    for (domain_name, cmd_name) in &all_commands {
        let variant = safe_ident(&format!("{}{}", domain_name, capitalize(cmd_name)));
        g.line(format!("    {},", variant));
    }
    g.close_block("}");
    g.blank();

    g.line("impl std::fmt::Display for CdpMethod {");
    g.line("    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {");
    g.line("        match self {");
    for (domain_name, cmd_name) in &all_commands {
        let variant = safe_ident(&format!("{}{}", domain_name, capitalize(cmd_name)));
        let method_str = format!("{}.{}", domain_name, cmd_name);
        g.line(format!(
            "            CdpMethod::{} => write!(f, {:?}),",
            variant, method_str
        ));
    }
    g.line("        }");
    g.line("    }");
    g.line("}");
    g.blank();

    // ---- Phase 4: CdpEvent enum (serde-tagged) ----
    g.line("// ------------------------------------------------------------------");
    g.line("// CdpEvent enum");
    g.line("// ------------------------------------------------------------------");
    g.blank();
    g.line("#[derive(Debug, Clone, serde::Deserialize)]");
    g.line("#[serde(tag = \"method\", content = \"params\")]");
    g.open_block("pub enum CdpEvent {");
    for (domain_name, evt_name) in &all_events {
        let variant = safe_ident(&format!("{}{}", domain_name, capitalize(evt_name)));
        let serde_name = format!("{}.{}", domain_name, evt_name);
        let mod_name = domain_to_module(domain_name);
        let evt_cap = capitalize(evt_name);
        let params_type = format!("{}::{}Params", mod_name, evt_cap);
        g.line(format!("    #[serde(rename = {:?})]", serde_name));
        g.line(format!("    {}({}),", variant, params_type));
    }
    g.close_block("}");
    g.blank();

    // ---- Phase 5: Command implementations ----
    g.line("// ------------------------------------------------------------------");
    g.line("// Command impls");
    g.line("// ------------------------------------------------------------------");
    g.blank();

    for (domain_name, cmd_name) in &all_commands {
        let domain = &all_domains[domain_name];
        let cmd_def = domain
            .commands
            .iter()
            .find(|c| c.name == *cmd_name)
            .unwrap();
        let mod_name = domain_to_module(domain_name);
        let cap_name = capitalize(cmd_name);

        let return_type = if cmd_def.returns.is_empty() {
            "()".to_string()
        } else {
            format!("{}::{}Return", mod_name, cap_name)
        };
        let full_method = format!("{}.{}", domain_name, cmd_name);

        g.line(format!(
            "impl Command for {}::{}Params {{",
            mod_name, cap_name
        ));
        g.line(format!("    type Return = {};", return_type));
        g.line(format!(
            "    fn method(&self) -> &'static str {{ {:?} }}",
            full_method
        ));
        g.line("}");
        g.blank();
    }

    // Write output
    let code = g.result();

    // Write to OUT_DIR (for cargo build)
    let out_dir = env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out_dir).join("cdp.rs"), &code).unwrap();

    // Also write to generated/ (for repo, committed)
    let generated_dir = manifest_dir.join("generated");
    std::fs::create_dir_all(&generated_dir).unwrap();
    std::fs::write(generated_dir.join("cdp.rs"), &code).unwrap();

    let stats = format!(
        "Generated {} domains, {} commands, {} events",
        domain_names.len(),
        all_commands.len(),
        all_events.len(),
    );
    println!("cargo:warning=cdp-protocol: {}", stats);
    println!(
        "cargo:warning=cdp-protocol: wrote OUT_DIR/cdp.rs + generated/cdp.rs ({} bytes)",
        code.len()
    );
}
