use std::io;
use lib::App;

fn main() -> io::Result<()> {
    ratatui::run(|terminal| App::new()?.run(terminal))
}
