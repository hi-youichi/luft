use std::env;
use std::fs;
use std::path::PathBuf;

fn parse_frontmatter(src: &str) -> (String, String, String) {
    let open = src.find("---").expect("SKILL.md must start with --- frontmatter fence");

    let search_from = open + 3;
    let close_rel = src[search_from..]
        .find("\n---")
        .expect("closing --- fence not found in SKILL.md");
    let close = search_from + close_rel;

    let fm_block = &src[open + 3..close];

    let after_close = &src[close + 4..];
    let body = after_close.trim_start_matches(['\r', '\n']);

    let mut name = String::new();
    let mut description = String::new();

    for line in fm_block.lines() {
        let trimmed = line.trim();
        if let Some(v) = trimmed.strip_prefix("name:") {
            name = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("description:") {
            description = v.trim().to_string();
        }
    }

    assert!(!name.is_empty(), "frontmatter: required field 'name' is missing or empty");
    assert!(
        !description.is_empty(),
        "frontmatter: required field 'description' is missing or empty"
    );

    (name, description, body.to_string())
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let skill_path = manifest_dir.join("src/skill/SKILL.md");

    println!("cargo:rerun-if-changed=src/skill/SKILL.md");

    let src = fs::read_to_string(&skill_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", skill_path.display()));

    let (name, description, body) = parse_frontmatter(&src);

    println!("cargo:rustc-env=SKILL_NAME={name}");
    println!("cargo:rustc-env=SKILL_DESCRIPTION={description}");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("skill_body.md"), &body)
        .unwrap_or_else(|e| panic!("failed to write skill_body.md to OUT_DIR: {e}"));

    println!("cargo:rustc-env=SKILL_BODY_PATH={}", out_dir.join("skill_body.md").display());
}
