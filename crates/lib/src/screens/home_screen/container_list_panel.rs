use serde::Deserialize;

use crate::screens::cli;

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct ContainerEntry {
    /// The name the container was created with (`--name`), or a UUID if it
    /// was given none. The CLI has no separate name.
    pub(crate) id: String,
    pub(crate) state: String,
    pub(crate) addr: String,
    /// The volumes it mounts, by name, in the order it mounts them.
    pub(crate) volumes: Vec<String>,
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

/// Every container, running or not. Blocks, so only the worker calls it.
pub(crate) fn fetch_containers() -> Result<Vec<ContainerEntry>, String> {
    parse_containers(&cli::output_of(&["ls", "--all", "--format", "json"])?)
}

/// One object of `container ls --format json`, cut down to what the panel
/// shows. Every field is optional, so a CLI release that drops or nulls one
/// leaves a blank cell rather than an empty list.
#[derive(Deserialize)]
struct ContainerJson {
    id: Option<String>,
    status: Option<StatusJson>,
    /// Kept untyped: only the mounts are read from it, and a mount of a
    /// kind this parser doesn't know must not cost the whole entry.
    configuration: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
struct StatusJson {
    state: Option<String>,
    networks: Option<Vec<NetworkJson>>,
}

#[derive(Deserialize)]
struct NetworkJson {
    #[serde(rename = "ipv4Address")]
    ipv4_address: Option<String>,
}

impl From<ContainerJson> for ContainerEntry {
    fn from(json: ContainerJson) -> Self {
        let status = json.status.unwrap_or_default();
        Self {
            id: json.id.unwrap_or_default(),
            state: status.state.unwrap_or_default(),
            // As the CLI's own table shows it: one CIDR address per
            // network, comma-separated.
            addr: status
                .networks
                .unwrap_or_default()
                .into_iter()
                .filter_map(|n| n.ipv4_address)
                .filter(|a| !a.is_empty())
                .collect::<Vec<_>>()
                .join(","),
            volumes: json
                .configuration
                .as_ref()
                .map(volume_names)
                .unwrap_or_default(),
        }
    }
}

/// The volumes among `configuration.mounts`, which look like
/// `{"type": {"volume": {"name": "pgdata", …}}, …}`. Bind mounts and tmpfs
/// have other keys under `type`. A name mounted twice is listed once.
fn volume_names(configuration: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mounts = configuration["mounts"].as_array().into_iter().flatten();
    for name in mounts.filter_map(|mount| mount["type"]["volume"]["name"].as_str()) {
        if !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// Reads the list one container at a time, so a single entry the parser
/// doesn't understand is skipped instead of hiding all the others.
///
/// The CLI lists containers in no particular order, and the order can change
/// when one is added, so they are sorted by name: otherwise every refresh
/// could shuffle the rows.
fn parse_containers(json: &str) -> Result<Vec<ContainerEntry>, String> {
    let items: Vec<serde_json::Value> = serde_json::from_str(json)
        .map_err(|e| format!("container ls printed something other than a JSON list: {e}"))?;

    let mut entries: Vec<ContainerEntry> = items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<ContainerJson>(item).ok())
        .map(ContainerEntry::from)
        // Without an ID there is nothing to start or stop.
        .filter(|entry| !entry.id.is_empty())
        .collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `container ls --all --format json` from CLI 1.4.1, trimmed to a few
    // fields per container. Stopped containers have no networks.
    const LIST: &str = r#"[
      {
        "configuration": {
          "id": "alpha",
          "image": { "reference": "docker.io/library/postgres:latest" },
          "resources": { "cpus": 4, "memoryInBytes": 1073741824 }
        },
        "id": "alpha",
        "status": { "networks": [], "state": "stopped" }
      },
      {
        "configuration": {
          "id": "bravo",
          "mounts": [
            {
              "destination": "/scratch",
              "options": [],
              "source": "tmpfs",
              "type": { "tmpfs": {} }
            },
            {
              "destination": "/data",
              "options": [],
              "source": "/Users/me/Library/Application Support/com.apple.container/volumes/9af1cc76-45ca-4205-9ed8-df97a4f132a8/volume.img",
              "type": {
                "volume": {
                  "cache": { "on": {} },
                  "format": "ext4",
                  "name": "9af1cc76-45ca-4205-9ed8-df97a4f132a8",
                  "sync": { "fsync": {} }
                }
              }
            },
            {
              "destination": "/src",
              "options": ["ro"],
              "source": "/Users/me/src",
              "type": { "virtiofs": {} }
            },
            {
              "destination": "/var/lib/postgresql",
              "options": [],
              "source": "/Users/me/Library/Application Support/com.apple.container/volumes/pgdata/volume.img",
              "type": { "volume": { "format": "ext4", "name": "pgdata" } }
            }
          ]
        },
        "id": "bravo",
        "status": {
          "networks": [
            {
              "hostname": "bravo",
              "ipv4Address": "192.168.64.3/24",
              "ipv4Gateway": "192.168.64.1",
              "ipv6Address": "fd13:49ef:eed1:ea03:fcf8:cfff:fe30:aeec/64",
              "macAddress": "fe:f8:cf:30:ae:ec",
              "mtu": 1280,
              "network": "default"
            }
          ],
          "startedDate": "2026-09-15T12:17:55Z",
          "state": "running"
        }
      }
    ]"#;

    #[test]
    fn parses_the_list() {
        let entries = parse_containers(LIST).unwrap();
        assert_eq!(
            entries,
            [
                ContainerEntry {
                    id: "alpha".to_string(),
                    state: "stopped".to_string(),
                    addr: String::new(),
                    volumes: vec![],
                },
                ContainerEntry {
                    id: "bravo".to_string(),
                    state: "running".to_string(),
                    addr: "192.168.64.3/24".to_string(),
                    volumes: vec![
                        "9af1cc76-45ca-4205-9ed8-df97a4f132a8".to_string(),
                        "pgdata".to_string(),
                    ],
                },
            ]
        );
        assert!(!entries[0].is_running());
        assert!(entries[1].is_running());
    }

    #[test]
    fn odd_mounts_are_skipped_without_losing_the_container() {
        let json = r#"[{"id": "web", "status": {"state": "running"}, "configuration": {"mounts": [
            {"type": "volume"},
            {"type": {"volume": {"name": 7}}},
            {"type": {"volume": {"name": ""}}},
            {"type": {"volume": {}}},
            {},
            "not a mount",
            {"type": {"volume": {"name": "cache"}}},
            {"type": {"volume": {"name": "cache"}}, "destination": "/again"}
        ]}}, {"id": "db", "configuration": {"mounts": {"name": "pgdata"}}}]"#;
        let entries = parse_containers(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "db");
        assert!(entries[0].volumes.is_empty());
        assert_eq!(entries[1].volumes, ["cache"]);
    }

    #[test]
    fn sorts_by_name_whatever_order_the_cli_uses() {
        let json = r#"[{"id": "web"}, {"id": "api"}, {"id": "db"}]"#;
        let ids: Vec<String> = parse_containers(json)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, ["api", "db", "web"]);
    }

    #[test]
    fn joins_the_addresses_of_every_network() {
        let json = r#"[{"id": "web", "status": {"state": "running", "networks": [
            {"ipv4Address": "192.168.64.3/24"},
            {"network": "no-address-yet"},
            {"ipv4Address": "10.0.0.2/16"}
        ]}}]"#;
        assert_eq!(
            parse_containers(json).unwrap()[0].addr,
            "192.168.64.3/24,10.0.0.2/16"
        );
    }

    #[test]
    fn missing_and_null_fields_leave_blank_cells() {
        let json = r#"[
            {"id": "no-status"},
            {"id": "null-status", "status": null},
            {"id": "null-state", "status": {"state": null, "networks": null}},
            {"id": "null-address", "status": {"state": "running", "networks": [{"ipv4Address": null}]}}
        ]"#;
        let entries = parse_containers(json).unwrap();
        assert_eq!(entries.len(), 4);
        assert!(entries.iter().all(|e| e.addr.is_empty()));
        let state = |id: &str| {
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.state.as_str())
        };
        assert_eq!(state("null-state"), Some(""));
        assert_eq!(state("null-address"), Some("running"));
    }

    #[test]
    fn an_entry_that_does_not_parse_is_skipped() {
        // `state` should be a string, and an entry needs an ID.
        let json = r#"[
            {"id": "odd", "status": {"state": 3}},
            {"status": {"state": "running"}},
            {"id": "fine", "status": {"state": "stopped"}}
        ]"#;
        let ids: Vec<String> = parse_containers(json)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, ["fine"]);
    }

    #[test]
    fn output_that_is_not_a_list_is_an_error() {
        assert_eq!(parse_containers("[]"), Ok(vec![]));
        assert!(parse_containers("").is_err());
        assert!(parse_containers("ID  IMAGE  STATE").is_err());
        assert!(parse_containers(r#"{"id": "web"}"#).is_err());
    }

    #[test]
    fn filter_cycles_between_all_and_running() {
        let entries = parse_containers(LIST).unwrap();
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
