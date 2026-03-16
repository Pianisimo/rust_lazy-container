use std::process::Command;

#[derive(Debug, Default)]
pub(crate) struct ContainerEntry {
    pub(crate) id: String,
    pub(crate) state: String,
    pub(crate) addr: String,
}

impl ContainerEntry {
    pub(crate) fn is_running(&self) -> bool {
        self.state == "running"
    }
}

pub(crate) fn fetch_containers() -> Vec<ContainerEntry> {
    let output = match Command::new("container").args(["ls", "-a"]).output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let header = match lines.next() {
        Some(h) => h,
        None => return vec![],
    };

    // Find byte offsets of each column header to slice data rows correctly.
    // ADDR can be empty for stopped containers, so we must use column positions
    // rather than splitting on whitespace.
    let offsets: Vec<usize> = ["ID", "IMAGE", "OS", "ARCH", "STATE", "ADDR", "CPUS"]
        .iter()
        .filter_map(|c| header.find(c))
        .collect();

    if offsets.len() < 7 {
        return vec![];
    }

    let image_start = offsets[1];
    let state_start = offsets[4];
    let addr_start = offsets[5];
    let cpus_start = offsets[6];

    lines
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let col = |s: usize, e: usize| -> String {
                line.get(s.min(line.len())..e.min(line.len()))
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };
            ContainerEntry {
                id: col(0, image_start),
                state: col(state_start, addr_start),
                addr: col(addr_start, cpus_start),
            }
        })
        .collect()
}
