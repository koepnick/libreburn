// SPDX-License-Identifier: AGPL-3.0-or-later
//! Every profile shipped in the repo must parse, and every HIL profile must cap power at zero.

use std::path::Path;

use libreburn_core::MachineProfile;

fn load_dir(dir: &str) -> Vec<MachineProfile> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(dir);
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "toml") {
            let text = std::fs::read_to_string(&path).unwrap();
            let profile = MachineProfile::from_toml(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            out.push(profile);
        }
    }
    assert!(!out.is_empty(), "no profiles found in {}", root.display());
    out
}

#[test]
fn shipped_profiles_parse() {
    load_dir("profiles");
}

#[test]
fn hil_profiles_cap_power_at_zero() {
    for p in load_dir("hil/profiles") {
        assert!(
            p.machine.hil,
            "{} is in hil/profiles but not marked hil",
            p.machine.name
        );
        assert_eq!(p.limits.max_power_pct, 0.0, "{}", p.machine.name);
        assert!(
            p.limits.hard.iter().any(|l| l == "max_power_pct"),
            "{}",
            p.machine.name
        );
    }
}
