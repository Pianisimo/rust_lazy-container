use std::process::Command;

use super::table;

#[derive(Debug, Default)]
pub(crate) struct ImageEntry {
    pub(crate) name: String,
    pub(crate) tag: String,
    pub(crate) digest: String,
    /// Every platform the image ships, in the order the CLI reports them.
    pub(crate) platforms: Vec<String>,
}

impl ImageEntry {
    /// What to hand the CLI to create a container from this image.
    pub(crate) fn reference(&self) -> String {
        if self.tag.is_empty() {
            self.name.clone()
        } else {
            format!("{}:{}", self.name, self.tag)
        }
    }

    /// The platform this host would actually run, plus a count of the others:
    /// `linux/arm64 +6`. Empty when the CLI did not report any.
    pub(crate) fn platform(&self) -> String {
        let preferred = match self
            .platforms
            .iter()
            .find(|p| p.ends_with(host_arch()))
            .or_else(|| self.platforms.first())
        {
            Some(p) => p,
            None => return String::new(),
        };

        match self.platforms.len() - 1 {
            0 => preferred.clone(),
            others => format!("{preferred} +{others}"),
        }
    }
}

/// The container CLI names architectures the way OCI does, not the way Rust does.
fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    }
}

pub(crate) fn fetch_images() -> Vec<ImageEntry> {
    // `--verbose` is what carries OS and ARCH; fall back to the plain table if
    // this CLI does not support it.
    let verbose = Command::new("container")
        .args(["image", "ls", "--verbose"])
        .output()
        .ok()
        .filter(|o| o.status.success());

    let output = match verbose {
        Some(o) => Some(o),
        None => Command::new("container")
            .args(["image", "ls"])
            .output()
            .ok()
            .filter(|o| o.status.success()),
    };

    match output {
        Some(o) => parse_images(&String::from_utf8_lossy(&o.stdout)),
        None => vec![],
    }
}

fn parse_images(text: &str) -> Vec<ImageEntry> {
    let mut lines = text.lines();
    let header = match lines.next() {
        Some(h) => h,
        None => return vec![],
    };

    let columns = table::columns(header);
    let name_span = match table::span(&columns, &["NAME"]) {
        Some(s) => s,
        None => return vec![],
    };
    let tag_span = table::span(&columns, &["TAG"]);
    let digest_span = table::span(&columns, &["INDEX DIGEST", "DIGEST"]);
    // Only the verbose table carries these.
    let os_span = table::span(&columns, &["OS"]);
    let arch_span = table::span(&columns, &["ARCH"]);

    let mut images: Vec<ImageEntry> = Vec::new();

    for line in lines.filter(|l| !l.trim().is_empty()) {
        let name = table::cell(line, name_span);
        let tag = tag_span.map(|s| table::cell(line, s)).unwrap_or_default();

        let platform = match (os_span, arch_span) {
            (Some(os), Some(arch)) => {
                let (os, arch) = (table::cell(line, os), table::cell(line, arch));
                if os.is_empty() || arch.is_empty() {
                    String::new()
                } else {
                    format!("{os}/{arch}")
                }
            }
            _ => String::new(),
        };

        // The verbose table repeats an image once per platform variant, so fold
        // the rows back into one entry per image.
        match images.iter_mut().find(|i| i.name == name && i.tag == tag) {
            Some(existing) => {
                if !platform.is_empty() && !existing.platforms.contains(&platform) {
                    existing.platforms.push(platform);
                }
            }
            None => images.push(ImageEntry {
                name,
                tag,
                digest: digest_span
                    .map(|s| table::cell(line, s))
                    .unwrap_or_default(),
                platforms: if platform.is_empty() {
                    vec![]
                } else {
                    vec![platform]
                },
            }),
        }
    }

    images
}

#[cfg(test)]
mod tests {
    use super::*;

    // `container image ls --verbose`: one row per platform variant, and the
    // arm v6/v7 rows collapse into a single `linux/arm`.
    const VERBOSE: &str = "\
NAME                                   TAG     INDEX DIGEST  OS     ARCH     VARIANT  FULL SIZE  CREATED                         MANIFEST DIGEST
alpine                                 edge    020dfcbaaf4c  linux  amd64             3,9 MB     2026-08-06T18:11:32.165151558Z  266f29255458
alpine                                 edge    020dfcbaaf4c  linux  arm      v6       3,6 MB     2026-08-06T18:11:22.67592895Z   789084105df9
alpine                                 edge    020dfcbaaf4c  linux  arm      v7       3,3 MB     2026-08-06T18:11:25.228645617Z  7db285f43ccc
alpine                                 edge    020dfcbaaf4c  linux  arm64    v8       4,2 MB     2026-08-06T18:11:21.275198184Z  42d86a317352
postgres                               latest  78481659c47e  linux  arm64    v8       160,9 MB   2026-04-22T01:34:52.523332957Z  f38a23be6b2c
ghcr.io/apple/containerization/vminit  0.31.0  59a94255a606  linux  arm64    v8       66,4 MB                                    9de0619d1f58
";

    // The plain table, which carries no platform at all.
    const PLAIN: &str = "\
NAME                                   TAG     DIGEST
alpine                                 edge    020dfcbaaf4c
postgres                               latest  78481659c47e
";

    #[test]
    fn folds_platform_rows_into_one_entry_per_image() {
        let images = parse_images(VERBOSE);
        assert_eq!(images.len(), 3);

        assert_eq!(images[0].name, "alpine");
        assert_eq!(images[0].tag, "edge");
        assert_eq!(images[0].digest, "020dfcbaaf4c");
        // v6 and v7 are both linux/arm, so they collapse.
        assert_eq!(
            images[0].platforms,
            ["linux/amd64", "linux/arm", "linux/arm64"]
        );

        assert_eq!(images[1].name, "postgres");
        assert_eq!(images[1].platforms, ["linux/arm64"]);
    }

    #[test]
    fn platform_column_prefers_the_host_architecture() {
        let images = parse_images(VERBOSE);

        // Whatever this host is, the shown platform matches it when available
        // and the count covers the rest.
        let shown = images[0].platform();
        let (head, tail) = shown.split_once(" +").expect("multi-platform image");
        assert!(images[0].platforms.contains(&head.to_string()));
        assert_eq!(tail, "2");
        if images[0].platforms.iter().any(|p| p.ends_with(host_arch())) {
            assert!(head.ends_with(host_arch()));
        }

        // A single-platform image shows no count.
        assert_eq!(images[1].platform(), "linux/arm64");
    }

    #[test]
    fn plain_table_parses_with_an_empty_platform() {
        let images = parse_images(PLAIN);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].name, "alpine");
        assert_eq!(images[0].digest, "020dfcbaaf4c");
        assert!(images[0].platforms.is_empty());
        assert_eq!(images[0].platform(), "");
    }

    #[test]
    fn tolerates_empty_output() {
        assert!(parse_images("").is_empty());
    }
}
