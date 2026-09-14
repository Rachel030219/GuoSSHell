use std::path::Path;

use alacritty_terminal::{
    Term,
    event::VoidListener,
    grid::Dimensions,
    term::{Config, TermMode},
    vte::ansi::Processor,
};

#[derive(Clone, Copy)]
struct TestSize;

impl Dimensions for TestSize {
    fn total_lines(&self) -> usize {
        3
    }

    fn screen_lines(&self) -> usize {
        3
    }

    fn columns(&self) -> usize {
        20
    }
}

#[test]
fn pinned_alacritty_exposes_the_required_public_terminal_api() {
    let mut terminal = Term::new(Config::default(), &TestSize, VoidListener);
    let mut processor: Processor = Processor::new();
    processor.advance(&mut terminal, b"public-api");
    terminal.resize(TestSize);
    let _ = terminal.grid();
    let _ = terminal.mode().contains(TermMode::SHOW_CURSOR);
    let _ = terminal.selection_to_string();
    let _ = terminal.damage();
    terminal.reset_damage();
}

#[test]
fn terminal_runtime_dependency_and_module_contract_is_exact() {
    let session = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = session.parent().unwrap().parent().unwrap();
    let manifest = std::fs::read_to_string(session.join("Cargo.toml")).unwrap();
    let lock = std::fs::read_to_string(root.join("Cargo.lock"))
        .unwrap()
        .replace("\r\n", "\n");
    let modules = std::fs::read_to_string(session.join("src/lib.rs")).unwrap();

    assert_eq!(
        manifest.matches("alacritty_terminal = \"=0.26.0\"").count(),
        1
    );
    assert_eq!(lock.matches("name = \"alacritty_terminal\"").count(), 1);
    assert!(lock.contains("name = \"alacritty_terminal\"\nversion = \"0.26.0\""));
    for forbidden in ["wezterm-term", "termwiz", "d69264df"] {
        assert!(!lock.contains(forbidden), "lockfile contains {forbidden}");
        assert!(
            !manifest.contains(forbidden),
            "manifest contains {forbidden}"
        );
    }
    for removed in ["wezterm_adapter", "wezterm_input", "wezterm_writer"] {
        assert!(!modules.contains(removed));
        assert!(!session.join(format!("src/{removed}.rs")).exists());
    }
}
