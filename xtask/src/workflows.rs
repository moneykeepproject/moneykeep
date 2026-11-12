//! `cargo xtask workflows`: static checks of the GitHub workflows.
//!
//! - upgrade-resilience-1 UR-8: the Windows CI job (`.github/workflows/windows.yml`,
//!   Batch U3). The job itself runs on GitHub; this checks that the file is well formed
//!   as far as can be told without a YAML parser, that its steps run the spec's
//!   commands in order, that every script and xtask task it names exists, and that
//!   third-party actions are pinned to a commit.
//! - macos-release.md W9 (module [`runner`]): only `release.yml` and
//!   `mac-runner-check.yml` may use the project's Mac (a self-hosted runner), only from
//!   a pushed tag or a hand run, only after the owner's approval, and only with GitHub's
//!   own pinned actions. The xtask tasks and packaging scripts `release.yml` names exist.

pub mod runner;

use std::path::Path;

pub const WINDOWS_WORKFLOW: &str = ".github/workflows/windows.yml";
pub const UPGRADE_SCRIPT: &str = "scripts/ci/windows-upgrade.ps1";
pub const FIXTURE_EXAMPLE: &str = "apps/moneykeep-server/examples/ci_fixture.rs";

/// `cargo xtask` tasks (main.rs's `match`).
pub const TASKS: [&str; 9] = [
    "ci",
    "layering",
    "names",
    "dist-capture",
    "capture-icons",
    "release",
    "installer",
    "phone-app",
    "workflows",
];

/// The release workflow (macos-release.md).
pub const RELEASE_WORKFLOW: &str = ".github/workflows/release.yml";

/// The spec's steps, in the order they must run.
pub const STEPS: [&str; 9] = [
    "cargo build --workspace --all-targets",
    "cargo test --workspace --exclude mk-views",
    "cargo xtask installer --no-wasm-opt",
    "cargo build -p moneykeep-server --example ci_fixture",
    "./scripts/ci/windows-upgrade.ps1 -Scenario pre-a1",
    "./scripts/ci/windows-upgrade.ps1 -Scenario custom-folder",
    "./scripts/ci/windows-upgrade.ps1 -Scenario reinstall",
    "./scripts/ci/windows-upgrade.ps1 -Scenario previous-release",
    "uses: actions/upload-artifact@",
];

pub fn check() -> Result<(), String> {
    let root = crate::release::workspace_root()?;
    check_in(&root)
}

fn read(root: &Path, rel: &str) -> Result<String, String> {
    std::fs::read_to_string(root.join(rel)).map_err(|e| format!("cannot read {rel}: {e}"))
}

pub fn check_in(root: &Path) -> Result<(), String> {
    let wf = read(root, WINDOWS_WORKFLOW)?;
    let mut errors = workflow_errors(&wf);
    // Every script and xtask task it names exists.
    for word in wf.split_whitespace() {
        let w = word.trim_matches(|c| c == '"' || c == '\'');
        if let Some(rel) = w.strip_prefix("./").filter(|r| r.starts_with("scripts/"))
            && !root.join(rel).is_file()
        {
            errors.push(format!("{rel} does not exist"));
        }
    }
    let release = read(root, RELEASE_WORKFLOW)?;
    for line in wf.lines().chain(release.lines()) {
        if let Some((_, rest)) = line.split_once("cargo xtask ") {
            // A task named in prose ends at its first character that isn't a name's.
            let task = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
            if !TASKS.contains(&task) {
                errors.push(format!("cargo xtask {task} is not a task"));
            }
        }
    }
    // The packaging scripts the release runs exist.
    for word in release.split_whitespace() {
        if let Some(rel) = word
            .strip_prefix("packaging/")
            .filter(|r| r.ends_with(".sh"))
        {
            let rel = format!("packaging/{rel}");
            if !root.join(&rel).is_file() {
                errors.push(format!("{rel} does not exist"));
            }
        }
    }
    // W9: the Mac runner rules, over every workflow.
    match runner::read_dir(&root.join(runner::WORKFLOWS_DIR)) {
        Ok(files) => errors.extend(runner::check_files(&files)),
        Err(e) => errors.push(e),
    }
    if !root.join(FIXTURE_EXAMPLE).is_file() {
        errors.push(format!("{FIXTURE_EXAMPLE} does not exist"));
    }
    match read(root, UPGRADE_SCRIPT) {
        Ok(s) => errors.extend(script_errors(&s)),
        Err(e) => errors.push(e),
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// The workflow's own problems.
pub fn workflow_errors(wf: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if wf.contains('\t') {
        errors.push("a tab (YAML indents with spaces only)".to_owned());
    }
    let has = |line: &str| wf.lines().any(|l| l.trim_end() == line);
    for line in [
        "on:",
        "  push:",
        "  pull_request:",
        "    runs-on: windows-latest",
    ] {
        if !has(line) {
            errors.push(format!("no line {line:?}"));
        }
    }
    // In order.
    let mut at = 0;
    for step in STEPS {
        match wf.get(at..).and_then(|rest| rest.find(step)) {
            Some(i) => at += i + step.len(),
            None => errors.push(format!("no step {step:?} (after the ones before it)")),
        }
    }
    // The logs are uploaded whatever happened before.
    if let Some(i) = wf.find("uses: actions/upload-artifact@") {
        let before = wf.get(..i).unwrap_or("");
        let step = before.rsplit("- name:").next().unwrap_or("");
        if !step.contains("if: always()") {
            errors.push("the upload step has no `if: always()`".to_owned());
        }
    }
    for line in wf.lines() {
        let l = line.trim().trim_start_matches("- ");
        let Some(action) = l.strip_prefix("uses:").map(str::trim) else {
            continue;
        };
        let Some((name, rev)) = action.split_once('@') else {
            errors.push(format!("{action}: no version"));
            continue;
        };
        // A `# vX.Y.Z` comment after the commit is allowed.
        let rev = rev.split_whitespace().next().unwrap_or("");
        // GitHub's own actions may use a tag; others are pinned to a full commit.
        let github = name.starts_with("actions/");
        let pinned = rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit());
        if !github && !pinned {
            errors.push(format!(
                "{action}: pin third-party actions to a full commit SHA"
            ));
        }
    }
    errors
}

/// The scenario script's problems: each scenario and the checks the spec names.
pub fn script_errors(s: &str) -> Vec<String> {
    let mut errors = Vec::new();
    for needle in [
        "'pre-a1' { Test-PreA1 }",
        "'custom-folder' { Test-CustomFolder }",
        "'reinstall' { Test-Reinstall }",
        "'previous-release' { Test-PreviousRelease }",
        "Set-StrictMode -Version Latest",
        "$ErrorActionPreference = 'Stop'",
        // Pre-A1: LocalSystem, the user's folder, a plain key.
        "New-Service -Name $Service",
        "$UserData = Join-Path $env:LOCALAPPDATA 'MoneyKeep\\data\\capture-server'",
        "& $Fixture pre-a1 --data $UserData",
        "$Account = 'NT SERVICE\\MoneyKeepCapture'",
        "$MachineData = Join-Path $MachineDir 'capture-server'",
        "& $Fixture check --url $Url --fixture $FixtureFile",
        "(Join-Path $UserData 'moved.json')",
        "Invoke-Setup $Setup @('/D=C:\\MK')",
        "'{\"reinstall_server\":true}'",
        "services apply --plan $plan --report $report --plan-sha256 $sha",
        "gh release download",
        "::notice title=Previous release::",
        "Copy-Item -Path $p -Destination $Out",
    ] {
        if !s.contains(needle) {
            errors.push(format!("{UPGRADE_SCRIPT} has no {needle:?}"));
        }
    }
    // Every function it calls is one it defines.
    for name in [
        "Test-PreA1",
        "Test-CustomFolder",
        "Test-Reinstall",
        "Test-PreviousRelease",
        "Reset-Machine",
        "New-StandIn",
        "Invoke-Setup",
        "Invoke-Uninstall",
        "Invoke-Plan",
        "Assert-Server",
        "Assert-Moved",
        "Save-Logs",
        "Get-Capture",
        "Get-InstallDir",
        "Get-One",
        "Fail",
    ] {
        if !s.contains(&format!("function {name}")) {
            errors.push(format!("{UPGRADE_SCRIPT} does not define {name}"));
        }
    }
    // Braces and brackets balance outside strings, here-strings and comments (a rough
    // syntax check; PowerShell itself parses it on the runner).
    let (mut braces, mut parens) = (0i64, 0i64);
    for line in code_lines(s) {
        for c in line.chars() {
            match c {
                '{' => braces += 1,
                '}' => braces -= 1,
                '(' => parens += 1,
                ')' => parens -= 1,
                _ => {}
            }
        }
    }
    if braces != 0 || parens != 0 {
        errors.push(format!(
            "{UPGRADE_SCRIPT}: unbalanced braces ({braces}) or parentheses ({parens})"
        ));
    }
    errors
}

/// The script's lines without the block comment, line comments and quoted text.
fn code_lines(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in s.lines() {
        let t = line.trim();
        if in_block {
            if t.contains("#>") {
                in_block = false;
            }
            continue;
        }
        if t.starts_with("<#") {
            in_block = !t.contains("#>");
            continue;
        }
        let mut code = String::new();
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for c in line.chars() {
            match quote {
                Some(q) => {
                    if escaped {
                        escaped = false;
                    } else if c == '`' && q == '"' {
                        escaped = true;
                    } else if c == q {
                        quote = None;
                    }
                }
                None => match c {
                    '#' => break,
                    '"' | '\'' => quote = Some(c),
                    _ => code.push(c),
                },
            }
        }
        out.push(code);
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// UR-8: the workflow file and its script pass the static check.
    #[test]
    fn ur8_windows_workflow_is_valid() {
        let root = crate::release::workspace_root().unwrap();
        if let Err(e) = check_in(&root) {
            panic!("{e}");
        }
        // TASKS is main.rs's list.
        let main = std::fs::read_to_string(root.join("xtask/src/main.rs")).unwrap();
        for t in TASKS {
            assert!(main.contains(&format!("\"{t}\" =>")), "{t}");
        }
    }

    /// UR-8: the check finds what it is for.
    #[test]
    fn ur8_the_check_catches_mistakes() {
        let root = crate::release::workspace_root().unwrap();
        let wf = std::fs::read_to_string(root.join(WINDOWS_WORKFLOW)).unwrap();
        assert!(workflow_errors(&wf).is_empty());
        let unpinned = wf.replace(
            "uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830",
            "uses: someone/cache@v4",
        );
        assert!(
            workflow_errors(&unpinned)
                .iter()
                .any(|e| e.contains("pin third-party")),
        );
        let pinned = wf.replace(
            "uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830",
            "uses: someone/cache@0123456789abcdef0123456789abcdef01234567",
        );
        assert!(workflow_errors(&pinned).is_empty());
        // Steps out of order.
        let swapped = wf
            .replace("-Scenario reinstall", "-Scenario TMP")
            .replace("-Scenario custom-folder", "-Scenario reinstall")
            .replace("-Scenario TMP", "-Scenario custom-folder");
        assert!(!workflow_errors(&swapped).is_empty());
        assert!(!workflow_errors(&wf.replace("if: always()", "")).is_empty());
        assert!(!workflow_errors(&wf.replacen("  ", "\t", 1)).is_empty());
        let s = std::fs::read_to_string(root.join(UPGRADE_SCRIPT)).unwrap();
        assert!(script_errors(&s).is_empty(), "{:?}", script_errors(&s));
        assert!(!script_errors(&s.replacen("{ Fail", "Fail", 1)).is_empty());
        assert!(!script_errors(&s.replace("function Assert-Moved", "function X")).is_empty());
    }
}
