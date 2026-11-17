//! Docker: container logs, build cache, old builds, unused images, stopped
//! containers and volumes, read and cleared through the docker CLI.
//!
//! Logs live in /var/lib/docker/containers, which is root's on Linux and
//! inside Docker Desktop's VM on macOS. A throwaway alpine container reaches
//! them on both, without a password, because the daemon runs as root.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::clutter::run_for;

/// Apps opened from Finder don't get the shell's PATH.
const BINS: [&str; 4] = [
    "/usr/bin/docker",
    "/usr/local/bin/docker",
    "/opt/homebrew/bin/docker",
    "/Applications/Docker.app/Contents/Resources/bin/docker",
];

/// Reads and empties container logs.
const HELPER: &str = "alpine:3";
const LOGS: &str = "/var/lib/docker/containers:/c";

/// The log limit "Keep logs small" sets: 3 files of 10 MB per container.
const MAX_SIZE: &str = "10m";
const MAX_FILE: &str = "3";

pub fn find() -> Option<PathBuf> {
    BINS.into_iter().map(PathBuf::from).find(|p| p.exists())
}

pub enum State {
    /// The CLI is there but the daemon doesn't answer.
    Stopped,
    Ready(Docker),
}

pub struct Docker {
    pub logs: Vec<Log>,
    pub cache: Part,
    /// Image names built more than once, largest first.
    pub builds: Vec<Builds>,
    /// Images no container uses, outside `builds`.
    pub unused: Freed,
    /// Stopped containers outside `builds`, with the images only they use.
    pub stopped: Freed,
    pub volumes: Part,
    /// Whether the daemon limits logs of new containers.
    pub capped: bool,
    /// Whether "Keep logs small" can set the limit: false when another log
    /// driver is set, which the json-file options would break.
    pub can_cap: bool,
}

pub struct Log {
    pub id: String,
    pub name: String,
    pub running: bool,
    /// Like "6 months ago", as docker prints it.
    pub created: String,
    /// The container has its own limit, so its log stays small.
    pub capped: bool,
    pub bytes: u64,
}

#[derive(Clone, Copy, Default)]
pub struct Part {
    pub count: usize,
    pub bytes: u64,
}

/// Containers and images to remove, by ID, and the space that frees.
#[derive(Clone, Default)]
pub struct Freed {
    pub containers: Vec<String>,
    pub images: Vec<String>,
    pub bytes: u64,
}

/// Every build of one image name. The newest and the running ones stay,
/// the rest go with the stopped containers made from them.
pub struct Builds {
    pub name: String,
    pub count: usize,
    /// Builds that go. Some may be gone as images already, kept alive only by
    /// their stopped containers, so this can exceed `old.images`.
    pub older: usize,
    /// Like ("myapp:latest", "built 2 hours ago") or (.., "running").
    pub kept: Vec<(String, String)>,
    pub old: Freed,
}

/// What to clear. Containers go before images, which they would hold.
#[derive(Clone, Default)]
pub struct Chosen {
    pub logs: Vec<String>,
    pub cache: bool,
    pub containers: Vec<String>,
    pub images: Vec<String>,
    pub volumes: bool,
}

struct Image {
    id: String,
    /// None for an untagged image, like the one a rebuild leaves behind.
    repo: Option<String>,
    tag: Option<String>,
    /// Sortable within one machine: "2026-11-05 23:30:31 +0530 IST".
    created: String,
    since: String,
    size: u64,
    /// What removing it frees: layers no other image shares.
    bytes: u64,
    /// Docker no longer lists it: with the containerd image store, a rebuild
    /// of the same tag drops the old image while its stopped containers keep
    /// its layers. Only removing those containers frees them.
    gone: bool,
}

struct Container {
    id: String,
    name: String,
    running: bool,
    since: String,
    created: String,
    capped: bool,
    /// The image's ID, and the name it was run with.
    image: String,
    image_name: String,
    /// Its own changes, and those plus its image's layers.
    bytes: u64,
    with_image: u64,
}

fn docker(bin: &Path, args: &[&str], limit: u64) -> Option<String> {
    run_for(Command::new(bin).args(args), Duration::from_secs(limit))
}

/// Everything Docker holds that can go. Blocks: the first read pulls the
/// small alpine image.
pub fn read(bin: &Path) -> State {
    if docker(bin, &["info", "--format", "{{.ServerVersion}}"], 8).is_none() {
        return State::Stopped;
    }
    let df = docker(
        bin,
        &[
            "system",
            "df",
            "--format",
            "{{.Type}}\t{{.TotalCount}}\t{{.Active}}\t{{.Reclaimable}}",
        ],
        20,
    )
    .unwrap_or_default();
    let part = |kind: &str| {
        let Some(line) = df.lines().find(|l| l.starts_with(&format!("{kind}\t"))) else {
            return Part::default();
        };
        let fields: Vec<&str> = line.split('\t').collect();
        let total: usize = fields.get(1).and_then(|n| n.parse().ok()).unwrap_or(0);
        let active: usize = fields.get(2).and_then(|n| n.parse().ok()).unwrap_or(0);
        let bytes = fields
            .get(3)
            .and_then(|r| r.split_whitespace().next())
            .and_then(crate::clutter::docker_bytes)
            .unwrap_or(0);
        Part {
            count: total.saturating_sub(active),
            bytes,
        }
    };
    let mut cache = part("Build Cache");
    // Build cache entries are never "active" in a way that keeps them.
    cache.count = df
        .lines()
        .find(|l| l.starts_with("Build Cache\t"))
        .and_then(|l| l.split('\t').nth(1)?.parse().ok())
        .unwrap_or(0);

    // Per image sizes, which the summary above lacks.
    let verbose: Value = docker(bin, &["system", "df", "-v", "--format", "{{json .}}"], 60)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let images = images(&verbose);
    let containers = containers(bin);
    let (builds, unused, stopped) = plan(images, &containers);

    let (capped, can_cap) = daemon_limit();
    State::Ready(Docker {
        logs: logs(bin, &containers),
        cache,
        builds,
        unused,
        stopped,
        volumes: part("Local Volumes"),
        capped,
        can_cap,
    })
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Sizes from `system df -v`, like "746.7MB".
fn bytes(value: &Value, key: &str) -> u64 {
    crate::clutter::docker_bytes(field(value, key)).unwrap_or(0)
}

fn images(verbose: &Value) -> Vec<Image> {
    let mut seen = HashSet::new();
    let known = |s: &str| (!s.is_empty() && s != "<none>").then(|| s.to_owned());
    verbose["Images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| seen.insert(field(i, "ID").to_owned()))
        .map(|i| Image {
            id: field(i, "ID").into(),
            repo: known(field(i, "Repository")),
            tag: known(field(i, "Tag")),
            created: field(i, "CreatedAt").into(),
            since: field(i, "CreatedSince").into(),
            size: bytes(i, "Size"),
            bytes: bytes(i, "UniqueSize"),
            gone: false,
        })
        .collect()
}

fn containers(bin: &Path) -> Vec<Container> {
    let Some(ps) = docker(
        bin,
        &[
            "ps",
            "-a",
            "--no-trunc",
            "--size",
            "--format",
            "{{.ID}}\t{{.Names}}\t{{.State}}\t{{.RunningFor}}\t{{.CreatedAt}}\t{{.Size}}",
        ],
        60,
    ) else {
        return Vec::new();
    };
    let ids: Vec<&str> = ps.lines().filter_map(|l| l.split('\t').next()).collect();
    let inspect = if ids.is_empty() {
        String::new()
    } else {
        let mut args = vec![
            "inspect",
            "--format",
            "{{.Id}}\t{{.Image}}\t{{.Config.Image}}\t{{index .HostConfig.LogConfig.Config \"max-size\"}}",
        ];
        args.extend(&ids);
        docker(bin, &args, 20).unwrap_or_default()
    };
    let inspect: HashMap<&str, Vec<&str>> = inspect
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Some((*f.first()?, f))
        })
        .collect();
    ps.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let id = f.next()?;
            let more = inspect.get(id);
            let at = |n: usize| more.and_then(|m| m.get(n)).map_or("", |s| s.trim());
            let name = f.next()?.into();
            let running = f.next()? == "running";
            let since = f.next()?.into();
            let created = f.next()?.into();
            let (bytes, with_image) = container_size(f.next().unwrap_or_default());
            Some(Container {
                id: id.into(),
                name,
                running,
                since,
                created,
                capped: !at(3).is_empty() && at(3) != "<no value>",
                image: at(1).into(),
                image_name: at(2).into(),
                bytes,
                with_image,
            })
        })
        .collect()
}

/// Every container with a log, largest first.
fn logs(bin: &Path, containers: &[Container]) -> Vec<Log> {
    if containers.is_empty() {
        return Vec::new();
    }
    let sizes = docker(
        bin,
        &[
            "run",
            "--rm",
            "--network",
            "none",
            "-v",
            &format!("{LOGS}:ro"),
            HELPER,
            "sh",
            "-c",
            "find /c -name '*-json.log*' -type f -exec stat -c '%s %n' {} +",
        ],
        90,
    )
    .unwrap_or_default();
    let mut logs: Vec<Log> = containers
        .iter()
        .filter_map(|c| {
            let bytes = log_bytes(&sizes, &c.id);
            (bytes > 0).then(|| Log {
                id: c.id.clone(),
                name: c.name.clone(),
                running: c.running,
                created: c.since.clone(),
                capped: c.capped,
                bytes,
            })
        })
        .collect();
    logs.sort_by_key(|l| std::cmp::Reverse(l.bytes));
    logs
}

/// The size of a container's log and its rotated files, from lines of
/// `stat -c '%s %n'` like "1234 /c/<id>/<id>-json.log.1".
fn log_bytes(stat: &str, id: &str) -> u64 {
    stat.lines()
        .filter_map(|line| {
            let (size, path) = line.split_once(' ')?;
            let dir = path.strip_prefix("/c/")?.split('/').next()?;
            (dir == id).then(|| size.parse::<u64>().ok()).flatten()
        })
        .sum()
}

/// From `ps --size`, like "4.1kB (virtual 29.3MB)".
fn container_size(text: &str) -> (u64, u64) {
    let parse = |s: &str| crate::clutter::docker_bytes(s.trim()).unwrap_or(0);
    let (own, rest) = text.split_once(" (virtual ").unwrap_or((text, ""));
    (parse(own), parse(rest.trim_end_matches(')')))
}

/// "myapp" from "myapp:latest", "localhost:5000/myapp:v2" or
/// "myapp@sha256:..". None when a container was run by image ID.
fn base_name(image: &str) -> Option<String> {
    let image = image.split('@').next()?;
    let slash = image.rfind('/').map_or(0, |i| i + 1);
    let name = match image[slash..].find(':') {
        Some(colon) => &image[..slash + colon],
        None => image,
    };
    (!name.is_empty() && !name.starts_with("sha256")).then(|| name.to_owned())
}

/// Splits images and stopped containers into old builds, unused images and
/// the remaining stopped containers, so nothing is counted twice.
fn plan(mut images: Vec<Image>, containers: &[Container]) -> (Vec<Builds>, Freed, Freed) {
    fn users<'a>(
        containers: &'a [Container],
        image: &'a str,
    ) -> impl Iterator<Item = &'a Container> {
        containers.iter().filter(move |c| c.image == image)
    }
    let on = |image| users(containers, image);

    let listed: HashSet<String> = images.iter().map(|i| i.id.clone()).collect();
    let mut gone: BTreeMap<&str, Image> = BTreeMap::new();
    for c in containers.iter().filter(|c| !listed.contains(&c.image)) {
        let image = gone.entry(&c.image).or_insert_with(|| Image {
            id: c.image.clone(),
            repo: None,
            tag: None,
            created: String::new(),
            since: c.since.clone(),
            size: c.with_image,
            bytes: c.with_image,
            gone: true,
        });
        if c.created > image.created {
            image.created = c.created.clone();
            image.since = c.since.clone();
        }
    }
    images.extend(gone.into_values());

    // An untagged image is named after the containers run from it: that is
    // how a rebuilt "myapp:latest" leaves its old build behind.
    let mut names: BTreeMap<String, Vec<&Image>> = BTreeMap::new();
    for image in &images {
        let name = image
            .repo
            .clone()
            .or_else(|| on(&image.id).find_map(|c| base_name(&c.image_name)));
        if let Some(name) = name {
            names.entry(name).or_default().push(image);
        }
    }

    let mut builds = Vec::new();
    let mut grouped = HashSet::new();
    for (name, mut list) in names {
        list.sort_by(|a, b| b.created.cmp(&a.created));
        // A gone build's size includes the base it shares with the builds
        // still listed. Their share of unique layers sets it apart.
        let unique = list
            .iter()
            .find(|i| !i.gone && i.size > 0)
            .map_or(1.0, |i| i.bytes as f64 / i.size as f64);
        let mut kept = Vec::new();
        let mut old = Freed::default();
        let mut older = 0;
        for (n, image) in list.iter().enumerate() {
            let label = match &image.tag {
                Some(tag) => format!("{name}:{tag}"),
                None => format!("{name}, untagged"),
            };
            if on(&image.id).any(|c| c.running) {
                kept.push((label, "running".into()));
            } else if n == 0 {
                kept.push((label, format!("built {}", lower_first(&image.since))));
            } else {
                older += 1;
                if image.gone {
                    old.bytes += (image.bytes as f64 * unique) as u64;
                } else {
                    old.images.push(image.id.clone());
                    old.bytes += image.bytes;
                }
                for c in on(&image.id) {
                    old.containers.push(c.id.clone());
                    old.bytes += c.bytes;
                }
            }
        }
        if older > 0 {
            grouped.extend(list.iter().map(|i| i.id.as_str()));
            builds.push(Builds {
                name,
                count: list.len(),
                older,
                kept,
                old,
            });
        }
    }
    builds.sort_by_key(|b| std::cmp::Reverse(b.old.bytes));

    let mut unused = Freed::default();
    let mut stopped = Freed::default();
    for image in images
        .iter()
        .filter(|i| !i.gone && !grouped.contains(i.id.as_str()))
    {
        let mut users = on(&image.id).peekable();
        if users.peek().is_none() {
            unused.images.push(image.id.clone());
            unused.bytes += image.bytes;
        } else if on(&image.id).all(|c| !c.running) {
            stopped.images.push(image.id.clone());
            stopped.bytes += image.bytes;
        }
    }
    for c in containers.iter().filter(|c| !c.running) {
        let on_old = builds.iter().any(|b| b.old.containers.contains(&c.id));
        if !on_old {
            stopped.containers.push(c.id.clone());
            stopped.bytes += c.bytes;
        }
    }
    (builds, unused, stopped)
}

/// Docker writes "About a minute ago" at the start of a sentence.
pub fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Clears what was chosen and returns whether every step worked.
pub fn clear(bin: &Path, chosen: &Chosen) -> bool {
    let mut ok = true;
    if !chosen.logs.is_empty() {
        // Truncated in place: Docker appends to the open file, so running
        // containers keep logging. Rotated files are deleted.
        let mut args = vec![
            "run",
            "--rm",
            "--network",
            "none",
            "-v",
            LOGS,
            HELPER,
            "sh",
            "-c",
            "for id in \"$@\"; do : > /c/$id/$id-json.log; rm -f /c/$id/$id-json.log.*; done",
            "sh",
        ];
        args.extend(chosen.logs.iter().map(String::as_str));
        ok &= docker(bin, &args, 60).is_some();
    }
    if chosen.cache {
        ok &= docker(bin, &["builder", "prune", "-af"], 300).is_some();
    }
    // Containers first: an image goes only once nothing points at it.
    for (args, ids) in [
        (["rm", "-v"], &chosen.containers),
        (["rmi", "-f"], &chosen.images),
    ] {
        for chunk in ids.chunks(200) {
            let mut all = args.to_vec();
            all.extend(chunk.iter().map(String::as_str));
            ok &= docker(bin, &all, 300).is_some();
        }
    }
    if chosen.volumes {
        ok &= docker(bin, &["volume", "prune", "-af"], 300).is_some();
    }
    ok
}

/// Docker's settings file.
fn settings() -> PathBuf {
    if cfg!(target_os = "macos") {
        crate::disk::home().join(".docker/daemon.json")
    } else {
        PathBuf::from("/etc/docker/daemon.json")
    }
}

/// Whether the daemon limits new containers' logs, and whether we can set
/// that limit.
fn daemon_limit() -> (bool, bool) {
    let text = std::fs::read_to_string(settings()).unwrap_or_default();
    let config: Value = serde_json::from_str(&text).unwrap_or(json!({}));
    let capped = config
        .pointer("/log-opts/max-size")
        .is_some_and(|v| v.is_string());
    let driver = config.get("log-driver").and_then(Value::as_str);
    let can_cap = matches!(driver, None | Some("json-file" | "local"));
    (capped, can_cap)
}

/// The settings with the log limit added, keeping everything else. None if
/// the file isn't JSON, so it is never overwritten with a guess.
fn with_limit(text: &str) -> Option<String> {
    let mut config: Map<String, Value> = if text.trim().is_empty() {
        Map::new()
    } else {
        serde_json::from_str(text).ok()?
    };
    let opts = config
        .entry("log-opts")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;
    opts.insert("max-size".into(), MAX_SIZE.into());
    opts.insert("max-file".into(), MAX_FILE.into());
    serde_json::to_string_pretty(&config).ok()
}

/// Sets the log limit and restarts Docker so it applies. On Linux this asks
/// for the password. Returns whether Docker came back with the limit.
pub fn keep_logs_small(bin: &Path) -> bool {
    let path = settings();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let Some(new) = with_limit(&text) else {
        return false;
    };
    let wrote = if cfg!(target_os = "macos") {
        // Docker Desktop keeps its engine settings here and applies them on
        // restart.
        path.parent()
            .is_some_and(|dir| std::fs::create_dir_all(dir).is_ok())
            && std::fs::write(&path, format!("{new}\n")).is_ok()
            && docker(bin, &["desktop", "restart"], 180).is_some()
    } else {
        Command::new("/usr/bin/pkexec")
            .args([
                "/bin/sh",
                "-c",
                "mkdir -p /etc/docker && printf '%s\\n' \"$1\" > /etc/docker/daemon.json && systemctl restart docker",
                "sh",
                &new,
            ])
            .status()
            .is_ok_and(|s| s.success())
    };
    wrote && wait_ready(bin) && daemon_limit().0
}

/// Starts Docker Desktop. Linux runs Docker as a system service, which
/// starts with the computer.
pub fn start(bin: &Path) -> bool {
    if cfg!(target_os = "macos") {
        let started = docker(bin, &["desktop", "start"], 180).is_some()
            || Command::new("/usr/bin/open")
                .args(["-a", "Docker"])
                .status()
                .is_ok_and(|s| s.success());
        started && wait_ready(bin)
    } else {
        false
    }
}

/// Waits up to two minutes for the daemon to answer.
fn wait_ready(bin: &Path) -> bool {
    (0..60).any(|_| {
        let up = docker(bin, &["info", "--format", "{{.ServerVersion}}"], 5).is_some();
        if !up {
            std::thread::sleep(Duration::from_secs(2));
        }
        up
    })
}

/// Sample data for `--demo`.
pub fn demo() -> Docker {
    let log = |name: &str, running: bool, created: &str, capped: bool, bytes: u64| Log {
        id: name.into(),
        name: name.into(),
        running,
        created: created.into(),
        capped,
        bytes,
    };
    let freed = |containers: usize, images: usize, bytes: u64| Freed {
        containers: (0..containers).map(|n| format!("c{n}")).collect(),
        images: (0..images).map(|n| format!("i{n}")).collect(),
        bytes,
    };
    let builds = |name: &str, count: usize, stopped: usize, bytes: u64| Builds {
        name: name.into(),
        count,
        older: count - 2,
        kept: vec![
            (format!("{name}:latest"), "built 40 minutes ago".into()),
            (format!("{name}:a91f3c2"), "running".into()),
        ],
        old: freed(stopped, count - 2, bytes),
    };
    Docker {
        logs: vec![
            log("search", true, "6 months ago", false, 6_903_000_000),
            log("cache", true, "6 months ago", false, 3_940_000),
            log("postgres", false, "20 hours ago", false, 1_200_000),
        ],
        cache: Part {
            count: 78,
            bytes: 2_122_000_000,
        },
        builds: vec![
            builds("myapp", 486, 409, 52_400_000_000),
            builds("myapp-worker", 71, 60, 4_900_000_000),
            builds("docs", 12, 3, 800_000_000),
        ],
        unused: freed(0, 10, 605_000_000),
        stopped: freed(3, 1, 1_200_000_000),
        volumes: Part {
            count: 11,
            bytes: 918_400_000,
        },
        capped: false,
        can_cap: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_the_limit_and_keeps_other_settings() {
        let text = r#"{"builder":{"gc":{"enabled":true}},"log-opts":{"compress":"true"}}"#;
        let new: Value = serde_json::from_str(&with_limit(text).unwrap()).unwrap();
        assert_eq!(new["builder"]["gc"]["enabled"], true);
        assert_eq!(new["log-opts"]["compress"], "true");
        assert_eq!(new["log-opts"]["max-size"], "10m");
        assert_eq!(new["log-opts"]["max-file"], "3");
        assert!(with_limit("").is_some());
        assert!(with_limit("not json").is_none());
    }

    #[test]
    fn sums_a_containers_log_files() {
        let stat = "100 /c/abc/abc-json.log\n50 /c/abc/abc-json.log.1\n7 /c/def/def-json.log";
        assert_eq!(log_bytes(stat, "abc"), 150);
        assert_eq!(log_bytes(stat, "def"), 7);
        assert_eq!(log_bytes(stat, "zzz"), 0);
    }

    fn image(id: &str, repo: Option<&str>, created: &str, bytes: u64) -> Image {
        Image {
            id: id.into(),
            repo: repo.map(Into::into),
            tag: repo.map(|_| "latest".into()),
            created: created.into(),
            since: "2 days ago".into(),
            size: bytes * 2,
            bytes,
            gone: false,
        }
    }

    fn container(id: &str, image: &str, name: &str, running: bool) -> Container {
        Container {
            id: id.into(),
            name: id.into(),
            running,
            since: String::new(),
            created: "2026-11-01".into(),
            capped: false,
            image: image.into(),
            image_name: name.into(),
            bytes: 1,
            with_image: 40,
        }
    }

    #[test]
    fn keeps_the_newest_and_running_builds() {
        let images = [
            // Rebuilt three times as myapp:latest; the old two are untagged.
            image("new", Some("myapp"), "2026-11-17", 100),
            image("mid", None, "2026-11-16", 100),
            image("old", None, "2026-11-15", 100),
            image("redis", Some("redis"), "2025-01-01", 50),
            image("alpine", Some("alpine"), "2026-01-01", 10),
        ];
        let containers = [
            container("c-mid", "mid", "myapp:latest", true),
            container("c-old", "old", "myapp", false),
            // An older build Docker no longer lists: half of it is unique,
            // like the builds it does list.
            container("c-gone", "gone", "myapp:latest", false),
            container("c-redis", "redis", "redis", false),
        ];
        let (builds, unused, stopped) = plan(images.into(), &containers);

        assert_eq!(builds.len(), 1);
        let myapp = &builds[0];
        assert_eq!((myapp.name.as_str(), myapp.count), ("myapp", 4));
        assert_eq!(myapp.kept.len(), 2, "newest and running stay");
        assert_eq!(myapp.older, 2);
        assert_eq!(myapp.old.images, ["old"], "a gone image can't be removed");
        assert_eq!(myapp.old.containers, ["c-old", "c-gone"]);
        assert_eq!(myapp.old.bytes, 101 + 20 + 1);

        assert_eq!(unused.images, ["alpine"]);
        assert_eq!(stopped.containers, ["c-redis"]);
        assert_eq!(stopped.images, ["redis"]);
    }

    #[test]
    fn names_images_like_docker_run_does() {
        assert_eq!(base_name("myapp:latest").as_deref(), Some("myapp"));
        assert_eq!(
            base_name("localhost:5000/team/app:v2").as_deref(),
            Some("localhost:5000/team/app")
        );
        assert_eq!(base_name("app@sha256:abc").as_deref(), Some("app"));
        assert_eq!(base_name("sha256:abc"), None);
    }

    #[test]
    fn reads_container_sizes() {
        assert_eq!(
            container_size("4.1kB (virtual 29.3MB)"),
            (4_100, 29_300_000)
        );
        assert_eq!(container_size("0B"), (0, 0));
    }
}
