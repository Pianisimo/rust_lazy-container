use serde::Deserialize;

use crate::screens::cli;

/// The registry that references without a host resolve to, unless
/// `container system property ls` says otherwise.
pub(crate) const DEFAULT_REGISTRY_DOMAIN: &str = "docker.io";

/// Docker Hub's names. Only there does a one-word image live under `library/`.
const DOCKER_HUB_DOMAINS: [&str; 2] = ["docker.io", "registry-1.docker.io"];

/// What the TAG column says for an image that has none, as in the CLI.
const NO_TAG: &str = "<none>";

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct ImageEntry {
    /// The full reference, such as `docker.io/library/alpine:edge`. This is
    /// what the CLI is handed.
    pub(crate) reference: String,
    /// The NAME and TAG columns of `container image ls`: `alpine` and `edge`.
    pub(crate) name: String,
    pub(crate) tag: String,
    /// The first 12 hex digits of the index digest.
    pub(crate) digest: String,
    /// Every platform the image ships, in the order the CLI reports them.
    pub(crate) platforms: Vec<String>,
}

impl ImageEntry {
    /// The image the way the list shows it: `alpine:edge`.
    pub(crate) fn label(&self) -> String {
        if self.tag == NO_TAG {
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

/// Every image, named as `container image ls` would name it given the
/// default `registry_domain`. Blocks, so only the worker calls it.
pub(crate) fn fetch_images(registry_domain: &str) -> Result<Vec<ImageEntry>, String> {
    parse_images(
        &cli::output_of(&["image", "ls", "--format", "json"])?,
        registry_domain,
    )
}

/// `[registry] domain` from the system properties, or `None` if it can't be
/// read. Blocks, so only the worker calls it.
pub(crate) fn fetch_registry_domain() -> Option<String> {
    parse_registry_domain(&cli::output_of(&["system", "property", "ls", "--format", "json"]).ok()?)
}

#[derive(Deserialize)]
struct PropertiesJson {
    registry: Option<RegistryJson>,
}

#[derive(Deserialize)]
struct RegistryJson {
    domain: Option<String>,
}

fn parse_registry_domain(json: &str) -> Option<String> {
    serde_json::from_str::<PropertiesJson>(json)
        .ok()?
        .registry?
        .domain
        .filter(|domain| !domain.is_empty())
}

/// One object of `container image ls --format json`, cut down to what the
/// panel shows. Every field is optional, so a CLI release that drops or
/// nulls one leaves a blank cell rather than an empty list.
#[derive(Deserialize)]
struct ImageJson {
    id: Option<String>,
    configuration: Option<ImageConfigurationJson>,
    variants: Option<Vec<VariantJson>>,
}

#[derive(Deserialize, Default)]
struct ImageConfigurationJson {
    name: Option<String>,
    descriptor: Option<DescriptorJson>,
}

#[derive(Deserialize)]
struct DescriptorJson {
    digest: Option<String>,
}

#[derive(Deserialize)]
struct VariantJson {
    platform: Option<PlatformJson>,
}

#[derive(Deserialize)]
struct PlatformJson {
    os: Option<String>,
    architecture: Option<String>,
}

/// Reads the list one image at a time, so a single entry the parser doesn't
/// understand is skipped instead of hiding all the others.
fn parse_images(json: &str, registry_domain: &str) -> Result<Vec<ImageEntry>, String> {
    let items: Vec<serde_json::Value> = serde_json::from_str(json)
        .map_err(|e| format!("container image ls printed something other than a JSON list: {e}"))?;

    Ok(items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<ImageJson>(item).ok())
        .filter_map(|image| image_entry(image, registry_domain))
        .collect())
}

/// `None` for an image without a reference, since there's nothing to create
/// a container from.
fn image_entry(json: ImageJson, registry_domain: &str) -> Option<ImageEntry> {
    let configuration = json.configuration.unwrap_or_default();
    let reference = configuration.name.filter(|name| !name.is_empty())?;
    let (name, tag) = split_tag(denormalize(&reference, registry_domain));

    let digest = configuration
        .descriptor
        .and_then(|d| d.digest)
        .or(json.id)
        .map(|digest| trim_digest(&digest))
        .unwrap_or_default();

    let mut platforms: Vec<String> = Vec::new();
    for platform in json
        .variants
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.platform)
    {
        let os = platform.os.unwrap_or_default();
        let arch = platform.architecture.unwrap_or_default();
        // Attestation manifests are listed as `unknown/unknown`; the CLI's
        // own table leaves them out. Arm v6 and v7 both read `linux/arm`, so
        // duplicates collapse.
        let platform = format!("{os}/{arch}");
        if os.is_empty()
            || arch.is_empty()
            || platform == "unknown/unknown"
            || platforms.contains(&platform)
        {
            continue;
        }
        platforms.push(platform);
    }

    Some(ImageEntry {
        name: name.to_string(),
        tag: tag.unwrap_or(NO_TAG).to_string(),
        reference,
        digest,
        platforms,
    })
}

/// A stored reference as `container image ls` prints it, which is how the
/// CLI's `ClientImage.denormalizeReference` does it: the default registry is
/// dropped, and on Docker Hub so is `library/`.
/// `docker.io/library/alpine:edge` becomes `alpine:edge`.
fn denormalize<'a>(reference: &'a str, registry_domain: &str) -> &'a str {
    match split_domain(reference) {
        Some((domain, rest)) if domain == registry_domain => {
            if DOCKER_HUB_DOMAINS.contains(&registry_domain) {
                rest.strip_prefix("library/").unwrap_or(rest)
            } else {
                rest
            }
        }
        _ => reference,
    }
}

/// The registry host at the front of a reference, found the way the CLI
/// finds one: a first path component that contains `.` or `:`, or starts
/// with `localhost`.
fn split_domain(reference: &str) -> Option<(&str, &str)> {
    let (first, rest) = reference.split_once('/')?;
    (first.starts_with("localhost") || first.contains(['.', ':'])).then_some((first, rest))
}

/// Splits `name[:tag][@digest]` into the name and tag. As in the CLI, a
/// digest wins: a reference that has one shows no tag.
fn split_tag(reference: &str) -> (&str, Option<&str>) {
    let (rest, has_digest) = match reference.split_once('@') {
        Some((rest, _)) => (rest, true),
        None => (reference, false),
    };
    // A colon before the last `/` belongs to a registry port, not a tag.
    let (name, tag) = match rest.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => (name, Some(tag)),
        _ => (rest, None),
    };
    (name, tag.filter(|_| !has_digest))
}

/// `sha256:020dfcbaaf4c…` becomes `020dfcbaaf4c`, as in the CLI's table.
fn trim_digest(digest: &str) -> String {
    let hex = digest.split_once(':').map_or(digest, |(_, hex)| hex);
    hex.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `container image ls --format json` from CLI 1.4.1, trimmed: the
    // unknown/unknown variants are attestation manifests, and the arm v6
    // and v7 variants share a platform.
    const LIST: &str = r#"[
      {
        "configuration": {
          "creationDate": "2026-08-06T18:11:18Z",
          "descriptor": {
            "digest": "sha256:020dfcbaaf4cc1078bf2d9c7ba31a8466e334061dcd2f248001d68f79e52c000",
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "size": 9234
          },
          "name": "docker.io/library/alpine:edge"
        },
        "id": "020dfcbaaf4cc1078bf2d9c7ba31a8466e334061dcd2f248001d68f79e52c000",
        "variants": [
          {
            "config": { "architecture": "amd64", "os": "linux", "history": [{ "created_by": "ADD rootfs / # buildkit" }] },
            "digest": "sha256:266f29255458134745f2bf588cb23ed1ed1768b96ff2580a05d70a8aba59e145",
            "platform": { "architecture": "amd64", "os": "linux" },
            "size": 3853714
          },
          { "digest": "sha256:51062f8856bf", "platform": { "architecture": "unknown", "os": "unknown" }, "size": 85283 },
          { "digest": "sha256:789084105df9", "platform": { "architecture": "arm", "os": "linux", "variant": "v6" }, "size": 3559404 },
          { "digest": "sha256:7db285f43ccc", "platform": { "architecture": "arm", "os": "linux", "variant": "v7" }, "size": 3270599 },
          { "digest": "sha256:42d86a317352", "platform": { "architecture": "arm64", "os": "linux", "variant": "v8" }, "size": 4191391 },
          { "digest": "sha256:ef57fd19a386", "platform": { "architecture": "unknown", "os": "unknown" }, "size": 85283 }
        ]
      },
      {
        "configuration": {
          "descriptor": { "digest": "sha256:78481659c47e862334611ccdaf7c369c986b3046da9857112f3b309114a65fb4" },
          "name": "docker.io/library/postgres:latest"
        },
        "id": "78481659c47e862334611ccdaf7c369c986b3046da9857112f3b309114a65fb4",
        "variants": [
          { "platform": { "architecture": "arm64", "os": "linux", "variant": "v8" }, "size": 160923337 }
        ]
      },
      {
        "configuration": {
          "creationDate": "1970-01-01T00:00:00Z",
          "descriptor": { "digest": "sha256:59a94255a606" },
          "name": "ghcr.io/apple/containerization/vminit:0.31.0"
        },
        "id": "59a94255a606",
        "variants": [
          { "platform": { "architecture": "arm64", "os": "linux", "variant": "v8" }, "size": 66383969 }
        ]
      }
    ]"#;

    fn parse(json: &str) -> Vec<ImageEntry> {
        parse_images(json, DEFAULT_REGISTRY_DOMAIN).unwrap()
    }

    #[test]
    fn names_images_the_way_the_cli_table_does() {
        let images = parse(LIST);
        let rows: Vec<(&str, &str, &str)> = images
            .iter()
            .map(|i| (i.name.as_str(), i.tag.as_str(), i.digest.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("alpine", "edge", "020dfcbaaf4c"),
                ("postgres", "latest", "78481659c47e"),
                (
                    "ghcr.io/apple/containerization/vminit",
                    "0.31.0",
                    "59a94255a606"
                ),
            ]
        );
    }

    #[test]
    fn keeps_the_full_reference_for_the_cli() {
        let images = parse(LIST);
        assert_eq!(images[0].reference, "docker.io/library/alpine:edge");
        assert_eq!(images[0].label(), "alpine:edge");
        assert_eq!(
            images[2].reference,
            "ghcr.io/apple/containerization/vminit:0.31.0"
        );
    }

    #[test]
    fn lists_each_real_platform_once() {
        let images = parse(LIST);
        assert_eq!(
            images[0].platforms,
            ["linux/amd64", "linux/arm", "linux/arm64"]
        );
        assert_eq!(images[1].platforms, ["linux/arm64"]);
    }

    #[test]
    fn platform_column_prefers_the_host_architecture() {
        let images = parse(LIST);

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
    fn missing_fields_leave_blank_cells() {
        let images = parse(
            r#"[
                {"configuration": {"name": "docker.io/library/bare:1"}},
                {"configuration": {"name": "docker.io/library/nulls:2", "descriptor": null}, "variants": null},
                {"configuration": {"name": "docker.io/library/half:3"}, "variants": [{"platform": {"os": "linux"}}, {}]}
            ]"#,
        );
        assert_eq!(images.len(), 3);
        for image in &images {
            assert_eq!(image.digest, "");
            assert!(image.platforms.is_empty());
            assert_eq!(image.platform(), "");
        }
        assert_eq!(images[2].label(), "half:3");
    }

    #[test]
    fn the_digest_falls_back_to_the_id() {
        let images = parse(
            r#"[{"id": "0123456789abcdef", "configuration": {"name": "docker.io/library/a:1"}}]"#,
        );
        assert_eq!(images[0].digest, "0123456789ab");
    }

    #[test]
    fn an_image_without_a_reference_is_skipped() {
        let images = parse(
            r#"[
                {"id": "no-configuration"},
                {"configuration": {"name": ""}},
                {"configuration": {"name": 7}},
                {"configuration": {"name": "docker.io/library/kept:1"}}
            ]"#,
        );
        let names: Vec<&str> = images.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["kept"]);
    }

    #[test]
    fn output_that_is_not_a_list_is_an_error() {
        assert_eq!(parse_images("[]", DEFAULT_REGISTRY_DOMAIN), Ok(vec![]));
        assert!(parse_images("NAME  TAG  DIGEST", DEFAULT_REGISTRY_DOMAIN).is_err());
    }

    #[test]
    fn denormalizes_like_the_cli() {
        let docker = DEFAULT_REGISTRY_DOMAIN;
        assert_eq!(
            denormalize("docker.io/library/alpine:edge", docker),
            "alpine:edge"
        );
        // Only one-word names live under `library/`.
        assert_eq!(
            denormalize("docker.io/grafana/grafana:11", docker),
            "grafana/grafana:11"
        );
        // Other registries keep their host.
        assert_eq!(
            denormalize("ghcr.io/apple/vminit:0.45.0", docker),
            "ghcr.io/apple/vminit:0.45.0"
        );
        assert_eq!(
            denormalize("localhost:5000/web:dev", docker),
            "localhost:5000/web:dev"
        );
        // With another default registry, that one is dropped instead, and
        // Docker Hub references keep their host.
        assert_eq!(
            denormalize("registry.example.com/web:1.0", "registry.example.com"),
            "web:1.0"
        );
        assert_eq!(
            denormalize("docker.io/library/alpine:edge", "registry.example.com"),
            "docker.io/library/alpine:edge"
        );
        assert_eq!(
            denormalize(
                "registry-1.docker.io/library/alpine:3",
                "registry-1.docker.io"
            ),
            "alpine:3"
        );
    }

    #[test]
    fn splits_the_tag_from_the_name() {
        assert_eq!(split_tag("alpine:edge"), ("alpine", Some("edge")));
        assert_eq!(split_tag("alpine"), ("alpine", None));
        // A registry port is not a tag.
        assert_eq!(
            split_tag("localhost:5000/web"),
            ("localhost:5000/web", None)
        );
        assert_eq!(
            split_tag("localhost:5000/web:dev"),
            ("localhost:5000/web", Some("dev"))
        );
        // A digest hides the tag, as it does in the CLI.
        let digest = "sha256:78481659c47e862334611ccdaf7c369c986b3046da9857112f3b309114a65fb4";
        assert_eq!(split_tag(&format!("postgres@{digest}")), ("postgres", None));
        assert_eq!(
            split_tag(&format!("postgres:18@{digest}")),
            ("postgres", None)
        );
    }

    #[test]
    fn an_untagged_image_shows_none_and_is_labelled_by_name() {
        let images = parse(
            r#"[{"configuration": {"name": "docker.io/library/postgres@sha256:78481659c47e862334611ccdaf7c369c986b3046da9857112f3b309114a65fb4"}}]"#,
        );
        assert_eq!(images[0].name, "postgres");
        assert_eq!(images[0].tag, "<none>");
        assert_eq!(images[0].label(), "postgres");
    }

    #[test]
    fn reads_the_registry_domain_from_the_properties() {
        let properties = r#"{"build": {"cpus": 2}, "registry": {"domain": "registry.example.com"}, "vminit": {}}"#;
        assert_eq!(
            parse_registry_domain(properties).as_deref(),
            Some("registry.example.com")
        );
        assert_eq!(parse_registry_domain(r#"{"registry": {}}"#), None);
        assert_eq!(
            parse_registry_domain(r#"{"registry": {"domain": ""}}"#),
            None
        );
        assert_eq!(
            parse_registry_domain("[registry]\ndomain = \"docker.io\""),
            None
        );
    }

    #[test]
    fn trims_digests_like_the_cli() {
        assert_eq!(trim_digest("sha256:020dfcbaaf4cc1078bf2"), "020dfcbaaf4c");
        assert_eq!(trim_digest("020dfcbaaf4cc1078bf2"), "020dfcbaaf4c");
        assert_eq!(trim_digest("sha256:abc"), "abc");
    }
}
