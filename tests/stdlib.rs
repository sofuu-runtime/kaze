// v0.9: the canonical robotics library composes, prepares, verifies and runs.

use std::path::Path;

use kaze::{isa, prepare_file, run_file, RunOptions};

fn write_mission(prefix: &str, body: &str) -> String {
    let dir =
        std::env::temp_dir().join(format!("kaze-stdlib-{}-{}", prefix, std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("mission.kaze");
    std::fs::write(&path, body).expect("write mission");
    path.to_string_lossy().to_string()
}

#[test]
fn every_standard_module_prepares_verifies_and_formats() {
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib");
    let mut seen = 0;
    for entry in std::fs::read_dir(&lib).expect("lib dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("kaze") {
            continue;
        }
        seen += 1;
        let name = path.to_string_lossy().to_string();
        let prepared =
            prepare_file(&name).unwrap_or_else(|e| panic!("{}: {}", name, e.render()));
        isa::verify(&prepared.module).unwrap_or_else(|why| panic!("{}: {}", name, why));

        let src = std::fs::read_to_string(&path).expect("read module");
        let once = kaze::fmt::format_source(&src, &name).expect("formats");
        let twice = kaze::fmt::format_source(&once, &name).expect("formats twice");
        assert_eq!(once, twice, "{} is not formatter-stable", name);
    }
    assert!(seen >= 3, "the standard library should have at least three modules");
}

#[test]
fn a_mission_composes_safety_and_survey() {
    let src = format!(
        "use \"{}/lib/safety.kaze\"\nuse \"{}/lib/survey.kaze\"\nneeds: log\n\naction main():\n    log \"composed\"\n",
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_MANIFEST_DIR")
    );
    let path = write_mission("compose", &src);

    let opts = RunOptions {
        grants: vec!["drone".to_string(), "log".to_string()],
        policy: Some(serde_json::json!({ "deny": [] })),
        backend: vec![("drone".into(), "sim".into())],
        entry: Some("eight_point_survey".into()),
        ..Default::default()
    };
    let report = run_file(&path, &opts).expect("the composed mission runs");
    assert_eq!(report.simulated_families, vec!["drone"]);
    assert_eq!(report.actions.len(), 12, "{:?}", report.actions);
}

#[test]
fn the_guardrail_allows_and_rejects() {
    let src = format!(
        "use \"{}/lib/guardrail.kaze\"\nneeds: log\n\naction main():\n    log \"guardrail\"\n",
        env!("CARGO_MANIFEST_DIR")
    );
    let path = write_mission("guard", &src);

    let allow = RunOptions {
        grants: vec!["policy".into()],
        policy: Some(serde_json::json!({ "deny": [] })),
        entry: Some("user_prompt".into()),
        args: serde_json::Map::from_iter([(
            "prompt".to_string(),
            serde_json::json!("what is the weather"),
        )]),
        ..Default::default()
    };
    let report = run_file(&path, &allow).expect("the filter runs");
    assert!(
        matches!(report.verdict, Some(kaze::Verdict::Allow(_))),
        "{:?}",
        report.verdict
    );

    let reject = RunOptions {
        grants: vec!["policy".into()],
        policy: Some(serde_json::json!({ "deny": [] })),
        entry: Some("user_prompt".into()),
        args: serde_json::Map::from_iter([(
            "prompt".to_string(),
            serde_json::json!("my api_key is sk-live-123"),
        )]),
        ..Default::default()
    };
    let report = run_file(&path, &reject).expect("the filter runs");
    assert!(
        matches!(report.verdict, Some(kaze::Verdict::Reject(_))),
        "{:?}",
        report.verdict
    );
}
