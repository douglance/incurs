//! Wraps the built `todo-desktop` executable in a macOS application bundle.
//!
//! ```sh
//! cargo build --release -p todo-desktop
//! cargo run -p todo-desktop --bin bundle-todo -- target/release/todo-desktop dist
//! open dist/Todo.app
//! ```

use std::path::PathBuf;

use incurs_app_gpui::bundle::MacBundle;

fn main() -> std::io::Result<()> {
    let mut arguments = std::env::args().skip(1);

    let Some(executable) = arguments.next().map(PathBuf::from) else {
        eprintln!("usage: bundle-todo <executable> [output-dir]");
        std::process::exit(2);
    };
    let output_dir = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("dist"));

    std::fs::create_dir_all(&output_dir)?;
    let app = MacBundle::new("Todo", "com.example.todo", executable)
        .version("1.0.0")
        .write(&output_dir)?;

    println!("{}", app.display());
    Ok(())
}
