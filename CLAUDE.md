# tiny-code

A Rust + egui desktop app that drives Claude Code in headless mode (`claude -p … --output-format stream-json`).
Specs: OpenSpec store `turbo-harness` in `~/coding/openspec` (`openspec list --store turbo-harness`).

## Gate (run before every commit)

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

`~/.cargo/bin` may be missing from non-interactive shells: `export PATH="$HOME/.cargo/bin:$PATH"`.

## Conventions

- egui 0.37: `eframe::App::ui(&mut Ui, …)`, `egui::Panel::{top,left,right,bottom}` with `.show(ui, …)`.
- egui 0.37 fonts (with `monochrome_emoji_fonts`) lack some common glyphs, e.g. `●` `↓` `↳` `→` `✓`; kittest
  fails on a missing glyph. Use ones known to render: `⏺` `✔` `✖` `⏸` `⏹` `⚠` `⬇` `↪` `…`.
- Decision logic lives in pure modules with unit tests; the UI only reads the model.
- Integration tests use a fake `claude` script under `tests/`; e2e tests (`tests/app_e2e.rs`) drive
  `app::App` through `egui_kittest` with a wrapper script around the fake.
- Commits: Conventional Commits (`feat(scope): …`).
