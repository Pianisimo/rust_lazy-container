//! Volumes, as `container volume ls` reports them. No panel lists them yet:
//! the delete dialog uses them to say which volumes go with a container, and
//! `D` deletes a container's anonymous ones after it, as `docker rm -v`
//! does. `container delete` has no option for that.

use std::collections::HashMap;

use serde::Deserialize;

use super::container_list_panel::fetch_containers;
use crate::screens::cli;

/// The label the CLI puts on the volume it makes for a mount that names
/// none, such as `-v /data`. The JSON has no other way to tell.
const ANONYMOUS_LABEL: &str = "com.apple.container.resource.anonymous";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VolumeEntry {
    pub(crate) name: String,
    /// Made for a mount that named no volume. Its name is a bare UUID.
    pub(crate) anonymous: bool,
}

/// Every volume. Blocks, so only the worker calls it.
pub(crate) fn fetch_volumes() -> Result<Vec<VolumeEntry>, String> {
    parse_volumes(&cli::output_of(&["volume", "ls", "--format", "json"])?)
}

/// One object of `container volume ls --format json`, cut down to what the
/// app uses. `id` is the volume's name.
#[derive(Deserialize)]
struct VolumeJson {
    id: Option<String>,
    configuration: Option<ConfigurationJson>,
}

#[derive(Deserialize)]
struct ConfigurationJson {
    /// Only the keys matter; the anonymous label's value is empty.
    labels: Option<HashMap<String, serde_json::Value>>,
}

/// Reads the list one volume at a time, like `parse_containers`, and sorts
/// it by name.
fn parse_volumes(json: &str) -> Result<Vec<VolumeEntry>, String> {
    let items: Vec<serde_json::Value> = serde_json::from_str(json).map_err(|e| {
        format!("container volume ls printed something other than a JSON list: {e}")
    })?;

    let mut volumes: Vec<VolumeEntry> = items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<VolumeJson>(item).ok())
        .filter_map(|json| {
            let name = json.id.filter(|id| !id.is_empty())?;
            let anonymous = json
                .configuration
                .and_then(|c| c.labels)
                .is_some_and(|labels| labels.contains_key(ANONYMOUS_LABEL));
            Some(VolumeEntry { name, anonymous })
        })
        .collect();
    volumes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(volumes)
}

/// What became of the volumes a deleted container left behind.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Cleanup {
    pub(crate) deleted: usize,
    /// Kept because another container mounts them.
    pub(crate) in_use: usize,
}

impl Cleanup {
    /// The notice for a successful `D`: "Purged web and 2 volumes".
    pub(crate) fn note(&self, id: &str) -> String {
        match (self.deleted, self.in_use) {
            (0, 0) => format!("Purged {id}"),
            (deleted, 0) => format!("Purged {id} and {}", volumes(deleted)),
            (0, in_use) => format!("Purged {id}, kept {} in use", volumes(in_use)),
            (deleted, in_use) => {
                format!("Purged {id} and {}, kept {in_use} in use", volumes(deleted))
            }
        }
    }
}

fn volumes(count: usize) -> String {
    match count {
        1 => "1 volume".to_string(),
        n => format!("{n} volumes"),
    }
}

/// Deletes `names`, the anonymous volumes of a container that has just been
/// deleted, one at a time. As `docker rm -v` does, it leaves alone a volume
/// that another container mounts, and says how many it kept that way. A name
/// that is gone, or isn't an anonymous volume, is skipped. Blocks, so only
/// the worker calls it.
pub(crate) fn delete_left_behind(names: &[String]) -> Result<Cleanup, String> {
    // Read after the container is gone, so its own mounts don't count.
    let containers = fetch_containers()?;
    let volumes = fetch_volumes()?;

    let mut cleanup = Cleanup::default();
    for name in names {
        if containers.iter().any(|c| c.volumes.contains(name)) {
            cleanup.in_use += 1;
            continue;
        }
        if !volumes.iter().any(|v| v.name == *name && v.anonymous) {
            continue;
        }
        // One call per volume: given a name that doesn't exist, the CLI
        // deletes none of the others either.
        let output = cli::run_detached(&["volume", "delete", "--", name])
            .map_err(|e| format!("couldn't run container: {e}"))?;
        if !output.success {
            return Err(volume_delete_failure(&output));
        }
        cleanup.deleted += 1;
    }
    Ok(cleanup)
}

/// Why `volume delete` failed. It logs the reason for each volume on a line
/// of its own, `failed to delete volume: ["error": …, "id": …]`, and then
/// ends with an `Error:` line that only names the volumes.
fn volume_delete_failure(output: &cli::Output) -> String {
    output
        .stderr
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("failed to delete volume"))
        .map(str::to_string)
        .unwrap_or_else(|| output.failure())
}

#[cfg(test)]
mod tests {
    use super::*;

    // `container volume ls --format json` from CLI 1.4.1: an anonymous
    // volume made by `-v /data`, and a named one made by `-v pgdata:/data`.
    const LIST: &str = r#"[
      {
        "configuration": {
          "creationDate": "2026-09-17T17:31:05Z",
          "driver": "local",
          "format": "ext4",
          "labels": { "com.apple.container.resource.anonymous": "" },
          "name": "9af1cc76-45ca-4205-9ed8-df97a4f132a8",
          "options": {},
          "sizeInBytes": 549755813888,
          "source": "/Users/me/Library/Application Support/com.apple.container/volumes/9af1cc76-45ca-4205-9ed8-df97a4f132a8/volume.img"
        },
        "id": "9af1cc76-45ca-4205-9ed8-df97a4f132a8"
      },
      {
        "configuration": {
          "creationDate": "2026-09-17T17:31:05Z",
          "driver": "local",
          "format": "ext4",
          "labels": {},
          "name": "pgdata",
          "options": {},
          "sizeInBytes": 549755813888,
          "source": "/Users/me/Library/Application Support/com.apple.container/volumes/pgdata/volume.img"
        },
        "id": "pgdata"
      }
    ]"#;

    fn volume(name: &str, anonymous: bool) -> VolumeEntry {
        VolumeEntry {
            name: name.to_string(),
            anonymous,
        }
    }

    #[test]
    fn parses_the_list_and_tells_anonymous_volumes_apart() {
        assert_eq!(
            parse_volumes(LIST).unwrap(),
            [
                volume("9af1cc76-45ca-4205-9ed8-df97a4f132a8", true),
                volume("pgdata", false),
            ]
        );
    }

    #[test]
    fn odd_entries_are_skipped_and_missing_labels_mean_named() {
        let json = r#"[
            {"id": "zeta"},
            {"id": "eta", "configuration": null},
            {"id": "theta", "configuration": {"labels": null}},
            {"id": "", "configuration": {"labels": {"com.apple.container.resource.anonymous": ""}}},
            {"configuration": {"name": "no-id"}},
            {"id": 3},
            {"id": "alpha", "configuration": {"labels": {"com.apple.container.resource.anonymous": null}}}
        ]"#;
        assert_eq!(
            parse_volumes(json).unwrap(),
            [
                volume("alpha", true),
                volume("eta", false),
                volume("theta", false),
                volume("zeta", false),
            ]
        );
    }

    #[test]
    fn output_that_is_not_a_list_is_an_error() {
        assert_eq!(parse_volumes("[]"), Ok(vec![]));
        assert!(parse_volumes("").is_err());
        assert!(parse_volumes("NAME  TYPE  DRIVER  OPTIONS").is_err());
    }

    #[test]
    fn the_note_counts_what_went_and_what_stayed() {
        let note = |deleted, in_use| Cleanup { deleted, in_use }.note("web");
        assert_eq!(note(0, 0), "Purged web");
        assert_eq!(note(1, 0), "Purged web and 1 volume");
        assert_eq!(note(3, 0), "Purged web and 3 volumes");
        assert_eq!(note(0, 1), "Purged web, kept 1 volume in use");
        assert_eq!(note(2, 1), "Purged web and 2 volumes, kept 1 in use");
    }

    #[test]
    fn a_failed_volume_delete_reports_the_logged_reason() {
        // What 1.4.1 prints for a volume that a container still mounts.
        let output = cli::Output::exited(
            1,
            "",
            "failed to delete volume: [\"error\": invalidArgument: \"volume 'pgdata' is currently in use and cannot be accessed by another container, or deleted\", \"id\": pgdata]\n\
             Error: delete failed for one or more volumes: [\"pgdata\"]\n",
        );
        assert_eq!(
            volume_delete_failure(&output),
            "failed to delete volume: [\"error\": invalidArgument: \"volume 'pgdata' is currently in use and cannot be accessed by another container, or deleted\", \"id\": pgdata]"
        );

        // Nothing logged: the error line is all there is.
        let output = cli::Output::exited(
            1,
            "",
            "Error: failed to delete one or more volumes: [\"gone\"]\n",
        );
        assert_eq!(
            volume_delete_failure(&output),
            "failed to delete one or more volumes: [\"gone\"]"
        );
    }
}
