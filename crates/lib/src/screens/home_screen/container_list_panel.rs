use std::process::Command;

use super::table;

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

/// Which containers the list panel shows. Cycled with `.`.
#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub(crate) enum ContainerFilter {
    #[default]
    All,
    RunningOnly,
}

impl ContainerFilter {
    pub(crate) fn next(self) -> Self {
        match self {
            ContainerFilter::All => ContainerFilter::RunningOnly,
            ContainerFilter::RunningOnly => ContainerFilter::All,
        }
    }

    pub(crate) fn matches(self, entry: &ContainerEntry) -> bool {
        match self {
            ContainerFilter::All => true,
            ContainerFilter::RunningOnly => entry.is_running(),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            ContainerFilter::All => "all",
            ContainerFilter::RunningOnly => "running",
        }
    }
}

pub(crate) fn fetch_containers() -> Vec<ContainerEntry> {
    let output = match Command::new("container").args(["ls", "-a"]).output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    parse_containers(&String::from_utf8_lossy(&output.stdout))
}

fn parse_containers(text: &str) -> Vec<ContainerEntry> {
    let mut lines = text.lines();
    let header = match lines.next() {
        Some(h) => h,
        None => return vec![],
    };

    let columns = table::columns(header);

    // The address column was named ADDR in older CLIs and IP since 1.2.
    let (image_start, _) = match table::span(&columns, &["IMAGE"]) {
        Some(s) => s,
        None => return vec![],
    };
    let state = match table::span(&columns, &["STATE"]) {
        Some(s) => s,
        None => return vec![],
    };
    let addr = table::span(&columns, &["IP", "ADDR"]);

    lines
        .filter(|l| !l.trim().is_empty())
        .map(|line| ContainerEntry {
            id: table::cell(line, (0, image_start)),
            state: table::cell(line, state),
            addr: addr.map(|s| table::cell(line, s)).unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `container ls -a` as of CLI 1.2: the address column is named IP, and
    // stopped rows leave it (and STARTED) empty.
    const IP_HEADER: &str = "\
ID        IMAGE                              OS     ARCH   STATE    IP            CPUS  MEMORY   STARTED
alpha     docker.io/library/postgres:latest  linux  arm64  stopped                4     1024 MB  
bravo     docker.io/library/redis:latest     linux  arm64  running  192.168.64.3  4     1024 MB  2h ago
";

    // Older CLIs called the same column ADDR and ended the row at CPUS.
    const ADDR_HEADER: &str = "\
ID        IMAGE                              OS     ARCH   STATE    ADDR          CPUS
alpha     docker.io/library/postgres:latest  linux  arm64  stopped                4
bravo     docker.io/library/redis:latest     linux  arm64  running  192.168.64.3  4
";

    #[test]
    fn parses_ip_column_layout() {
        let entries = parse_containers(IP_HEADER);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "alpha");
        assert_eq!(entries[0].state, "stopped");
        assert_eq!(entries[0].addr, "");
        assert_eq!(entries[1].id, "bravo");
        assert_eq!(entries[1].state, "running");
        assert_eq!(entries[1].addr, "192.168.64.3");
    }

    #[test]
    fn parses_legacy_addr_column_layout() {
        let entries = parse_containers(ADDR_HEADER);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].addr, "192.168.64.3");
        assert!(entries[1].is_running());
        assert!(!entries[0].is_running());
    }

    #[test]
    fn tolerates_empty_and_headerless_output() {
        assert!(parse_containers("").is_empty());
        assert!(parse_containers("no containers\n").is_empty());
    }

    #[test]
    fn filter_cycles_between_all_and_running() {
        let entries = parse_containers(IP_HEADER);
        let filter = ContainerFilter::default();
        assert_eq!(filter, ContainerFilter::All);
        assert_eq!(entries.iter().filter(|e| filter.matches(e)).count(), 2);

        let filter = filter.next();
        assert_eq!(filter, ContainerFilter::RunningOnly);
        let running: Vec<_> = entries.iter().filter(|e| filter.matches(e)).collect();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, "bravo");

        assert_eq!(filter.next(), ContainerFilter::All);
    }
}
