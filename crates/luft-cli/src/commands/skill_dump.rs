use std::path::Path;

use anyhow::Result;
use luft_skills::WORKFLOW_SKILL;

pub fn skill_dump(dir: &Path) -> Result<()> {
    let count = luft_skills::write_to_dir(dir, &WORKFLOW_SKILL)?;
    println!(
        "Skill '{}' dumped to {} ({} files)",
        WORKFLOW_SKILL.name,
        dir.display(),
        count
    );
    Ok(())
}
