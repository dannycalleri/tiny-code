use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use turbo_harness::app::App;

fn fake_claude_for(fixture: &str, name: &str) -> PathBuf {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let wrapper = std::env::temp_dir().join(format!(
        "turbo-harness-e2e-{}-{name}.sh",
        std::process::id()
    ));
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nFAKE_FIXTURE='{}' exec sh '{}' \"$@\"\n",
            tests.join("fixtures").join(fixture).display(),
            tests.join("fake_claude.sh").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    wrapper
}

fn harness(claude: PathBuf) -> Harness<'static, App> {
    let app = App::new(std::env::temp_dir(), claude.display().to_string());
    Harness::builder()
        .with_size([900.0, 1400.0])
        .build_ui_state(
            |ui, app: &mut App| {
                app.poll();
                app.show(ui);
            },
            app,
        )
}

fn send(harness: &mut Harness<'_, App>, prompt: &str) {
    harness.get_by_role(Role::MultilineTextInput).click();
    harness.run();
    harness
        .get_by_role(Role::MultilineTextInput)
        .type_text(prompt);
    harness.run();
    harness.get_by_role_and_label(Role::Button, "Send").click();
    harness.run();
}

fn wait_for_the_run(harness: &mut Harness<'_, App>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.state().is_running() {
        assert!(Instant::now() < deadline, "the run did not end");
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
    harness.run();
}

#[test]
fn a_run_shows_markdown_and_a_card_per_tool_call() {
    let claude = fake_claude_for("tools.jsonl", "tools");
    let mut harness = harness(claude.clone());

    send(&mut harness, "Read notes.txt");
    wait_for_the_run(&mut harness);

    harness.get_by_label("Summary");
    harness.get_by_label("Bash");
    // One `ls` is inline code in the Markdown answer; the other is the Bash card summary.
    assert_eq!(harness.get_all_by_label("ls").count(), 2);
    harness.get_by_label("/home/user/demo/notes.txt");
    harness.get_by_label("/home/user/demo/missing.txt");
    assert_eq!(harness.get_all_by_label("Read").count(), 2);
    harness.get_by_label("✔ done");
    assert_eq!(harness.get_all_by_label("✔").count(), 2);
    harness.get_by_label("✖");
    let _ = std::fs::remove_file(claude);
}

#[test]
fn a_long_result_is_collapsed_and_expands_on_click() {
    let claude = fake_claude_for("long_result.jsonl", "long");
    let mut harness = harness(claude.clone());

    send(&mut harness, "Print 2000 lines");
    wait_for_the_run(&mut harness);

    assert!(harness.query_by_label("Show all 2000 lines").is_none());
    harness.get_by_label("Output · 2000 lines").click();
    harness.run();
    harness.get_by_label("Show all 2000 lines");
    let _ = std::fs::remove_file(claude);
}

#[test]
fn a_result_saved_to_a_file_shows_its_size_and_a_link() {
    let claude = fake_claude_for("saved_output.jsonl", "saved");
    let mut harness = harness(claude.clone());

    send(&mut harness, "List /usr/bin");
    wait_for_the_run(&mut harness);

    harness.get_by_label("Output · 55.9KB · preview");
    harness.get_by_label("Open full output");
    assert!(
        harness
            .query_by_label_contains("persisted-output")
            .is_none()
    );
    let _ = std::fs::remove_file(claude);
}
