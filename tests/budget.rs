// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! Size and hygiene gates: module line budgets, a `///` "why" doc on every item, the dependency
//! ledger and the unsafe ledger. Std only; it reads the sources as text and does not import the
//! crate. Run `cargo test --test budget -- --nocapture` to see the line count per module.

use std::{collections::BTreeMap, fs, path::Path};

/// Percentage by which a module may exceed its line budget before the gate fails.
const TOLERANCE_PCT: usize = 10;

/// Item keywords that must carry a `///` doc, after any visibility prefix.
const ITEMS: &[&str] = &["fn ", "struct ", "enum ", "trait ", "type ", "const ", "static ", "mod "];

/// Reads a file relative to the package root; the gates are meaningless without their inputs.
fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Lists `(file name, source)` for every module in the flat `src/` layout, sorted by name.
fn modules() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out: Vec<(String, String)> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let is_rs = Path::new(&name).extension().is_some_and(|ext| ext == "rs");
            is_rs.then(|| (name.clone(), read(&format!("src/{name}"))))
        })
        .collect();
    out.sort();
    out
}

/// The part of a module that the gates look at: everything before its `#[cfg(test)]` module,
/// which by convention is the last item of a file.
fn production(src: &str) -> impl Iterator<Item = &str> {
    src.lines().take_while(|line| line.trim() != "#[cfg(test)]")
}

/// Counts code lines: no blank or comment lines, no test module, no compiled-in data between
/// `// budget: data` and `// budget: end`.
fn code_lines(src: &str) -> usize {
    let mut in_data = false;
    let mut count = 0;
    for line in production(src).map(str::trim) {
        match line {
            "// budget: data" => in_data = true,
            "// budget: end" => in_data = false,
            _ if in_data || line.is_empty() || line.starts_with("//") => {}
            _ => count += 1,
        }
    }
    count
}

/// Parses `key number` lines of `docs/budget.txt`; `#` starts a comment.
fn parse_budget(text: &str) -> BTreeMap<String, usize> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('#').next()?.split_whitespace();
            let key = fields.next()?;
            Some((key.to_owned(), fields.next()?.parse().ok()?))
        })
        .collect()
}

/// Compares measured module lines with the budget; returns one message per violation.
fn check_loc(budget: &BTreeMap<String, usize>, measured: &[(String, usize)]) -> Vec<String> {
    let mut errors = Vec::new();
    for (name, lines) in measured {
        match budget.get(name) {
            None => errors.push(format!("{name}: no line in docs/budget.txt")),
            Some(&max) if lines * 100 > max * (100 + TOLERANCE_PCT) => {
                errors.push(format!("{name}: {lines} lines > budget {max} + {TOLERANCE_PCT} %"));
            }
            Some(_) => {}
        }
    }
    let total: usize = measured.iter().map(|(_, lines)| lines).sum();
    match budget.get("total") {
        None => errors.push("docs/budget.txt: no total".to_owned()),
        Some(&max) if total > max => errors.push(format!("total {total} lines > cap {max}")),
        Some(_) => {}
    }
    errors
}

/// Finds items without a `///` doc line above them (attributes may sit in between).
fn undocumented(file: &str, src: &str) -> Vec<String> {
    let lines: Vec<&str> = production(src).map(str::trim).collect();
    let mut errors = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let rest = strip_visibility(line);
        let rest = rest.strip_prefix("unsafe ").unwrap_or(rest);
        if !ITEMS.iter().any(|kw| rest.starts_with(kw)) {
            continue;
        }
        let doc = lines[..i].iter().rev().find(|prev| !prev.starts_with("#["));
        if !doc.is_some_and(|prev| prev.starts_with("///")) {
            errors.push(format!("{file}:{}: no /// doc: {line}", i + 1));
        }
    }
    errors
}

/// Removes a leading `pub`, `pub(crate)`, `pub(super)` or `pub(in …)` from a trimmed line.
fn strip_visibility(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix("pub(") {
        return rest.split_once(") ").map_or(line, |(_, item)| item);
    }
    line.strip_prefix("pub ").unwrap_or(line)
}

/// The table a manifest header names, without a `target.<triple or 'cfg(…)'>.` prefix.
fn table(header: &str) -> &str {
    let Some(rest) = header.strip_prefix("target.") else { return header };
    let after = match rest.chars().next() {
        Some(q @ ('\'' | '"')) => rest[1..].split_once(q).map_or("", |(_, r)| r),
        _ => rest.split_once('.').map_or("", |(_, r)| r),
    };
    after.trim_start_matches('.')
}

/// Keys of every dependency table of a Cargo manifest: `[dependencies]`,
/// `[build-dependencies]`, their `[target.….]` forms and `[dependencies.<key>]` headers.
fn dependency_keys(manifest: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut in_table = false;
    for line in manifest.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            let t = table(header.trim());
            in_table = t == "dependencies" || t == "build-dependencies";
            let named = t.strip_prefix("dependencies.").or(t.strip_prefix("build-dependencies."));
            keys.extend(named.map(|key| key.trim_matches(['"', '\'']).to_owned()));
        } else if let Some((key, _)) = line.split_once('=').filter(|_| in_table) {
            let key = key.split('.').next().unwrap_or_default().trim().trim_matches(['"', '\'']);
            if !key.is_empty() && !key.starts_with('#') {
                keys.push(key.to_owned());
            }
        }
    }
    keys
}

/// `<file>:<item>` of every `allow(unsafe_code)` or `expect(unsafe_code)` attribute, also
/// across lines; the item is the identifier after the next item keyword or `impl`, and an
/// inner attribute (`#![…]`) names the whole file as `<file>`.
fn unsafe_sites(file: &str, src: &str) -> Vec<String> {
    let mut sites = Vec::new();
    let (mut attr, mut depth, mut open) = (String::new(), 0, false);
    for line in src.lines().map(str::trim) {
        if depth == 0 && (line.starts_with("#[") || line.starts_with("#![")) {
            attr.clear();
        }
        if depth > 0 || line.starts_with("#[") || line.starts_with("#![") {
            attr.push_str(line);
            depth = (depth + line.matches('[').count()).saturating_sub(line.matches(']').count());
            if depth == 0 {
                let opens = attr.contains("unsafe_code")
                    && (attr.contains("allow(") || attr.contains("expect("));
                if opens && attr.starts_with("#![") {
                    sites.push(file.to_owned());
                } else {
                    open |= opens;
                }
            }
            continue;
        }
        if open && !line.is_empty() && !line.starts_with("//") {
            let rest = strip_visibility(line);
            let rest = rest.strip_prefix("unsafe ").unwrap_or(rest);
            let item = ITEMS.iter().chain(&["impl "]).find_map(|kw| rest.strip_prefix(kw));
            let name = item.unwrap_or(rest).trim_start();
            let end = name.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(name.len());
            sites.push(format!("{file}:{}", &name[..end]));
            open = false;
        }
    }
    sites
}

/// `<file>:<item>` of every `## <n>. <file>:<item>` entry in the "Our code" part of the
/// unsafe ledger.
fn ledger_entries(ledger: &str) -> Vec<String> {
    ledger
        .lines()
        .skip_while(|line| line.trim() != "# Our code")
        .skip(1)
        .take_while(|line| !line.starts_with("# "))
        .filter_map(|line| line.strip_prefix("## "))
        .map(|entry| entry.split_once(". ").map_or(entry, |(_, site)| site).trim().to_owned())
        .collect()
}

/// Gate 1: every module within its line budget and the sum within the hard cap.
#[test]
#[allow(clippy::print_stdout, reason = "the per-module table is the report PRs quote")]
fn module_line_budgets() {
    let budget = parse_budget(&read("docs/budget.txt"));
    let measured: Vec<(String, usize)> =
        modules().into_iter().map(|(name, src)| (name, code_lines(&src))).collect();
    for (name, lines) in &measured {
        let max = budget.get(name).map_or_else(|| "-".to_owned(), ToString::to_string);
        println!("{name:<12} {lines:>5} / {max}");
    }
    let errors = check_loc(&budget, &measured);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

/// Gate 2: every item states why it exists.
#[test]
fn items_have_why_docs() {
    let errors: Vec<String> =
        modules().iter().flat_map(|(name, src)| undocumented(name, src)).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

/// Gate 3: every direct dependency has a row in `docs/deps.md`.
#[test]
fn dependencies_are_ledgered() {
    let ledger = read("docs/deps.md");
    let missing: Vec<String> = dependency_keys(&read("Cargo.toml"))
        .into_iter()
        .filter(|key| !ledger.contains(&format!("| `{key}` |")))
        .collect();
    assert!(missing.is_empty(), "no docs/deps.md row for: {}", missing.join(", "));
}

/// Gate 4: every `allow`/`expect(unsafe_code)` site has its `<file>:<item>` entry in
/// `docs/unsafe.md`, and no more.
#[test]
fn unsafe_sites_match_ledger() {
    let mut sites: Vec<String> = modules()
        .iter()
        .flat_map(|(name, src)| unsafe_sites(&format!("src/{name}"), src))
        .collect();
    let mut entries = ledger_entries(&read("docs/unsafe.md"));
    sites.sort();
    entries.sort();
    assert_eq!(sites, entries, "unsafe_code sites in src/ vs \"Our code\" entries");
}

#[test]
fn code_lines_skips_comments_blanks_data_and_tests() {
    let src = "// c\n/// d\nfn a() {\n\n    1;\n}\n// budget: data\nconst X: u8 = 1;\n\
               // budget: end\nconst Y: u8 = 2;\n#[cfg(test)]\nmod tests {}\n";
    assert_eq!(code_lines(src), 4);
}

#[test]
fn check_loc_fails_over_tolerance_missing_line_and_total() {
    let budget = parse_budget("a.rs 100 # comment\nb.rs 10\ntotal 120\n");
    assert!(check_loc(&budget, &[("a.rs".into(), 110), ("b.rs".into(), 10)]).is_empty());
    assert_eq!(check_loc(&budget, &[("a.rs".into(), 111)]).len(), 1);
    assert_eq!(check_loc(&budget, &[("c.rs".into(), 1)]).len(), 1);
    assert_eq!(check_loc(&budget, &[("a.rs".into(), 110), ("b.rs".into(), 11)]).len(), 1);
}

#[test]
fn undocumented_finds_items_without_doc() {
    let src = "/// why\n#[derive(Debug)]\npub struct A;\npub(crate) fn b() {}\n\
               impl A {\n    /// why\n    const C: u8 = 0;\n    unsafe fn d() {}\n}\n\
               #[cfg(test)]\nmod tests { fn e() {} }\n";
    let errors = undocumented("x.rs", src);
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].starts_with("x.rs:4:") && errors[1].starts_with("x.rs:8:"));
}

#[test]
fn ledgers_parse() {
    let manifest = "[package]\nname = \"x\"\n[dependencies]\n# why\nclap = \"4\"\n\
                    lz4_flex = { version = \"0.14\" }\n\n[lints.rust]\nx = 1\n";
    assert_eq!(dependency_keys(manifest), ["clap", "lz4_flex"]);
    let ledger = "# unsafe\n## not counted\n# Our code\n## 1. src/a.rs:x\n## 2. src/b.rs:y\n\
                  # Dependencies\n## c\n";
    assert_eq!(ledger_entries(ledger), ["src/a.rs:x", "src/b.rs:y"]);
}

#[test]
fn dependency_keys_cover_target_build_and_dotted_tables() {
    let manifest = "[dependencies]\nclap = \"4\"\nlibc.workspace = true\n\
                    [target.'cfg(target_feature = \"sse4.1\")'.dependencies]\nrustix = \"1\"\n\
                    [target.x86_64-unknown-linux-gnu.build-dependencies]\ncc = \"1\"\n\
                    [dependencies.nix]\nversion = \"0.30\"\n[dev-dependencies]\ntempfile = \"3\"\n\
                    [target.'cfg(unix)'.dev-dependencies]\nproptest = \"1\"\n";
    assert_eq!(dependency_keys(manifest), ["clap", "libc", "rustix", "cc", "nix"]);
}

#[test]
fn unsafe_sites_include_expect_and_multi_line_attributes() {
    let src = "#[allow(unsafe_code, reason = \"a\")]\nmod counting {}\n\
               #[expect(unsafe_code, reason = \"b\")]\n/// doc\n#[inline]\npub unsafe fn ffi() {}\n\
               #[expect(\n    unsafe_code,\n    reason = \"c [d]\"\n)]\nunsafe impl Send for X {}\n\
               #[deny(unsafe_code)]\nfn safe() {}\n";
    assert_eq!(
        unsafe_sites("src/x.rs", src),
        ["src/x.rs:counting", "src/x.rs:ffi", "src/x.rs:Send"]
    );
    assert_eq!(unsafe_sites("src/y.rs", "#![allow(unsafe_code, reason = \"e\")]\n"), ["src/y.rs"]);
}
