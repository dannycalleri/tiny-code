use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use turbo_harness::app::App;
use turbo_harness::store::{self, IndexEntry, Store};

fn fake_claude_for(fixture: &str, name: &str) -> PathBuf {
    fake_claude_with(fixture, name, "")
}

fn fake_claude_with(fixture: &str, name: &str, env: &str) -> PathBuf {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let wrapper = std::env::temp_dir().join(format!(
        "turbo-harness-e2e-{}-{name}.sh",
        std::process::id()
    ));
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n{env} FAKE_FIXTURE='{}' exec sh '{}' \"$@\"\n",
            tests.join("fixtures").join(fixture).display(),
            tests.join("fake_claude.sh").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _ = std::fs::remove_dir_all(wrapper.with_extension("store"));
    wrapper
}

fn store_of(claude: &Path) -> Store {
    Store::open(&claude.with_extension("store"), &std::env::temp_dir())
}

fn harness(claude: PathBuf) -> Harness<'static, App> {
    harness_sized(claude, [900.0, 1400.0])
}

fn harness_sized(claude: PathBuf, size: [f32; 2]) -> Harness<'static, App> {
    let app = App::new(
        std::env::temp_dir(),
        claude.display().to_string(),
        store_of(&claude),
    );
    Harness::builder()
        .with_size(size)
        .with_max_steps(16)
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
    harness.get_by_label("Last run: $0.23 · 9.4 s · 4 turns");
    assert_eq!(harness.get_all_by_label("✔").count(), 2);
    harness.get_by_label("✖");

    let index = store_of(&claude).index();
    let [entry] = &index[..] else {
        panic!("{index:?}")
    };
    assert_eq!(
        (
            entry.title.as_str(),
            format!("{:.4}", entry.cost_usd.unwrap()),
            entry.status.as_str()
        ),
        ("Read notes.txt", "0.2279".to_string(), "done")
    );
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

#[test]
fn a_subagent_card_is_collapsed_and_shows_its_steps_on_click() {
    let claude = fake_claude_for("subagents.jsonl", "subagents");
    let mut harness = harness(claude.clone());

    send(&mut harness, "Start a nested subagent");
    wait_for_the_run(&mut harness);

    assert!(harness.query_by_label("Read").is_none());
    harness
        .get_by_label("✔ Agent · Nested grep then read main.rs · 5 steps")
        .click();
    harness.run();
    harness.get_by_label("Read");
    harness.get_by_label("/home/user/demo/main.rs");
    harness.get_by_label("✔ Agent · Grep TODO in subdemo · 2 steps");
    assert!(harness.query_by_label_contains("task_").is_none());
    let _ = std::fs::remove_file(claude);
}

#[test]
fn an_orphan_step_is_marked_and_steps_under_any_call_stay_visible() {
    let claude = fake_claude_for("orphan.jsonl", "orphan");
    let mut harness = harness(claude.clone());

    send(&mut harness, "List and read");
    wait_for_the_run(&mut harness);

    harness.get_by_label("⚠ orphan: its parent call is unknown");
    harness.get_by_label("/home/user/demo/a.txt");
    harness.get_by_label("✔ Bash · ls · 0 steps").click();
    harness.run();
    harness.get_by_label("Nested under Bash");
    let _ = std::fs::remove_file(claude);
}

#[test]
fn clicking_a_nested_call_in_the_outline_opens_and_shows_its_card() {
    let claude = fake_claude_for("subagents.jsonl", "outline");
    let mut harness = harness_sized(claude.clone(), [1200.0, 420.0]);

    send(&mut harness, "Start a nested subagent");
    wait_for_the_run(&mut harness);

    assert!(
        harness
            .query_by_role_and_label(Role::Label, "Read")
            .is_none()
    );
    harness.get_by_role_and_label(Role::Button, "Read").click();
    harness.run();

    let card = harness.get_by_role_and_label(Role::Label, "Read").rect();
    let top = harness
        .get_by_label(&std::env::temp_dir().display().to_string())
        .rect()
        .bottom();
    let bottom = harness.get_by_role(Role::MultilineTextInput).rect().top();
    assert!(
        top < card.top() && card.bottom() < bottom,
        "{card:?} not in {top}..{bottom}"
    );
    let _ = std::fs::remove_file(claude);
}

#[test]
fn a_past_session_reopens_and_continues_and_a_new_one_starts_fresh() {
    const ID: &str = "9e26b157-1db2-4ff3-99fc-8737a92f55ba";
    let argv = std::env::temp_dir().join(format!("turbo-harness-e2e-{}-argv", std::process::id()));
    let claude = fake_claude_with(
        "hello.jsonl",
        "history",
        &format!("FAKE_ARGV='{}'", argv.display()),
    );
    let store = store_of(&claude);
    let mut log = store.log_writer(Some(ID));
    log.write(&store::prompt_line("Say hello", 1));
    for line in include_str!("fixtures/hello.jsonl").lines() {
        log.write(line);
    }
    log.write(&store::exit_line(Some(0), &[], false));
    store
        .save_entry(IndexEntry {
            session_id: ID.into(),
            title: "Say hello".into(),
            started_at: 1,
            updated_at: 1,
            cost_usd: Some(0.05),
            status: "done".into(),
        })
        .unwrap();
    let mut harness = harness(claude.clone());

    assert!(harness.query_by_label("Hello, nice to meet you!").is_none());
    harness
        .get_by_role_and_label(Role::Button, "Say hello")
        .click();
    harness.run();
    harness.get_by_label("Hello, nice to meet you!");

    send(&mut harness, "And again");
    wait_for_the_run(&mut harness);
    let args = std::fs::read_to_string(&argv).unwrap();
    assert!(args.contains(&format!("--resume\n{ID}\n")), "{args}");
    assert_eq!(
        harness.get_all_by_label("Hello, nice to meet you!").count(),
        2
    );

    harness
        .get_by_role_and_label(Role::Button, "New session")
        .click();
    harness.run();
    assert!(harness.query_by_label("Hello, nice to meet you!").is_none());
    send(&mut harness, "Fresh start");
    wait_for_the_run(&mut harness);
    assert!(!std::fs::read_to_string(&argv).unwrap().contains("--resume"));
    let _ = std::fs::remove_file(argv);
    let _ = std::fs::remove_file(claude);
}
