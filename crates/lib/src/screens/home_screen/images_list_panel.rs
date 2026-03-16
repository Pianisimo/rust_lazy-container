use std::process::Command;

#[derive(Debug, Default)]
pub(crate) struct ImageEntry {
    pub(crate) display: String,
}

pub(crate) fn fetch_images() -> Vec<ImageEntry> {
    let output = match Command::new("container").args(["image", "ls"]).output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    lines.next(); // skip header

    lines
        .filter(|l| !l.trim().is_empty())
        .map(|line| ImageEntry {
            display: line.trim().to_string(),
        })
        .collect()
}
