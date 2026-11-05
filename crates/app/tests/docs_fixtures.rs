//! Generates the Rust-side payloads of a synthetic three-repository demo workspace so the
//! production web bundle can be screenshotted for the README. Not a behaviour test.

use git_gotten_app::Backend;
use git_gotten_protocol::{ChildNode, Op, RepoSelection, ResetMode};
use git_gotten_testutil as testutil;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const PLANNER1: &str = "pub fn plan(seed: u64) -> Trajectory {\n    let burn = burn_window(seed);\n    Trajectory { burn, windows: 3 }\n}\n";
const PLANNER2: &str = "pub fn plan(seed: u64) -> Trajectory {\n    let burn = burn_window(seed);\n    let cached = CACHE.get(&seed).copied().unwrap_or(burn);\n    Trajectory { burn: cached, windows: 3 }\n}\n";
const PLANNER3: &str = "pub fn plan(seed: u64) -> Trajectory {\n    let burn = burn_window(seed);\n    let cached = CACHE.get(&seed).copied().unwrap_or(burn);\n    Trajectory { burn: cached, windows: 4 }\n}\n";
const PLANNER5: &str = "pub fn plan(seed: u64, drift: f64) -> Trajectory {\n    let burn = burn_window(seed).with_drift(drift);\n    let cached = CACHE.get(&seed).copied().unwrap_or(burn);\n    Trajectory { burn: cached, windows: 4 }\n}\n";
const TELE1: &str = "pub fn stream(sock: Socket) {\n    sock.send(&frame())\n}\n";
const TELE2: &str = "pub fn stream(sock: Socket) {\n    for frame in batch(8) {\n        sock.send(&frame)\n    }\n}\n";
const TELE3: &str = "pub fn stream(sock: Socket) {\n    for frame in batch(8) {\n        if !frame.is_stale(86_400) {\n            sock.send(&frame)\n        }\n    }\n}\n";
const TELE4: &str = "pub fn stream(sock: Socket, retry: u8) {\n    for frame in batch(16) {\n        if !frame.is_stale(86_400) {\n            sock.send_with(&frame, retry)\n        }\n    }\n}\n";

fn write_json(dir: &Path, name: &str, value: &Value) {
    std::fs::write(dir.join(name), serde_json::to_string(value).expect("serialize"))
        .expect("write");
}

fn write_map(dir: &Path, name: &str, map: &Map<String, Value>) {
    write_json(dir, name, &Value::Object(map.clone()));
}

fn json(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("serialize")
}

/// One commit on `main` made through a throwaway clone of the bare remote, so the local
/// checkout is behind.
fn push_from_remote(remote: &Path, scratch: &Path) {
    std::fs::remove_dir_all(scratch).ok();
    std::fs::create_dir_all(scratch).expect("clone dir");
    testutil::git_run(scratch, &["init", "-q", "-b", "main"]);
    testutil::git_run(scratch, &["remote", "add", "origin", remote.to_str().unwrap()]);
    testutil::git_run(scratch, &["config", "user.name", "Remote Bot"]);
    testutil::git_run(scratch, &["config", "user.email", "bot@orbit.example"]);
    testutil::git(scratch, &["fetch", "-q", "origin"]);
    testutil::git(scratch, &["reset", "--hard", "-q", "origin/main"]);
    std::fs::create_dir_all(scratch.join("ops")).expect("ops dir");
    std::fs::write(scratch.join("ops/oncall.md"), "paging rules\n").expect("write");
    testutil::git(scratch, &["add", "ops/oncall.md"]);
    testutil::git(scratch, &["commit", "-q", "-m", "document the paging rules"]);
    testutil::git(scratch, &["push", "-q", "origin", "main"]);
}

/// Scratch roots for the documented screenshots, under the OS temp dir.
fn docs_root(sub: &str) -> PathBuf {
    std::env::temp_dir().join("gg-docs").join(sub)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("copy dir");
    for entry in std::fs::read_dir(from).expect("read source") {
        let entry = entry.expect("entry");
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        match entry.file_type().expect("file type") {
            ft if ft.is_dir() => copy_dir(&src, &dst),
            // Symlinks are resolved by `fs::copy`; the fixtures never rely on them.
            _ => {
                std::fs::copy(&src, &dst).expect("copy file");
            }
        }
    }
}

/// Copy a fixture repository to a stable path, so the screenshots show readable repository names.
/// The fixture's own cleanup still deletes the original.
fn relocate(fx: &testutil::Fixture, name: &str) -> PathBuf {
    let root = docs_root("repos");
    let target = root.join(name);
    std::fs::create_dir_all(&root).expect("repos dir");
    std::fs::remove_dir_all(&target).ok();
    copy_dir(Path::new(fx.id()), &target);
    target.canonicalize().expect("canonical target")
}

/// Run on demand: `cargo test -p git-gotten-app --test docs_fixtures -- --ignored`.
#[tokio::test]
#[ignore = "regenerates the README screenshot fixtures; not part of the normal suite"]
async fn dump_docs_fixtures() {
    std::fs::remove_dir_all(docs_root("repos")).ok();
    let out_dir = docs_root("fixtures");
    let out: &Path = &out_dir;
    std::fs::create_dir_all(out).expect("fixtures dir");

    // ── orbit: the showcase repository ───────────────────────────────────────
    let mut orbit = testutil::Fixture::new("orbit");
    orbit.git(&["config", "user.name", "Nadia Fox"]);
    orbit.git(&["config", "user.email", "nadia@orbit.example"]);
    let base = 1_759_000_000_i64;
    orbit.commit_file_at("README.md", "# Orbit\n\nMission planner.\n", "Orbit: mission planner", base);
    orbit.commit_file_at("src/planner.rs", PLANNER1, "plan trajectories from a seeded RNG", base + 3_600);
    orbit.git(&["tag", "-a", "v1.0.0", "-m", "Orbit 1.0.0"]);
    orbit.git(&["checkout", "-q", "-b", "feature/telemetry"]);
    orbit.commit_file_at("src/telemetry.rs", TELE1, "stream telemetry over UDP", base + 7_200);
    orbit.commit_file_at("src/telemetry.rs", TELE2, "batch telemetry frames", base + 10_800);
    orbit.git(&["checkout", "-q", "main"]);
    orbit.commit_file_at("src/planner.rs", PLANNER2, "cache trajectory lookups", base + 14_400);
    orbit.git(&["merge", "--no-ff", "-q", "--no-edit", "-m", "Merge branch 'feature/telemetry'", "feature/telemetry"]);
    orbit.commit_file_at("docs/operations.md", "Runbook:\n1. arm\n2. burn\n3. confirm\n", "add the operator guide", base + 18_000);
    orbit.git(&["tag", "-a", "v1.2.0", "-m", "Orbit 1.2.0"]);
    orbit.git(&["checkout", "-q", "-b", "release/1.3"]);
    orbit.commit_file_at("docs/roadmap.md", "1.3: adaptive burns\n", "draft the 1.3 roadmap", base + 21_600);
    orbit.git(&["checkout", "-q", "main"]);

    let origin = orbit.add_origin("main");
    orbit.commit_file_at("src/planner.rs", PLANNER3, "reuse burn windows", base + 25_200);
    orbit.commit_file_at("src/telemetry.rs", TELE3, "drop telemetry frames older than 24h", base + 28_800);
    push_from_remote(&origin, &docs_root("remote-clone"));
    orbit.git(&["fetch", "-q", "origin"]);

    orbit.write_file("src/planner.rs", "pub fn plan(seed: u64) -> Trajectory {\n    let burn = burn_window(seed).adaptive();\n    Trajectory { burn, windows: 4 }\n}\n");
    orbit.git(&["stash", "push", "-q", "-m", "wip: adaptive burn windows"]);
    orbit.write_file("src/planner.rs", PLANNER5);
    orbit.write_file("src/telemetry.rs", TELE4);
    orbit.write_file("notes/scratch.md", "notes that were never committed\n");
    orbit.write_file("notes/checklist.md", "1. fuel\n2. cool\n3. go\n");
    orbit.add("notes/checklist.md"); // a staged addition: the Files panel shows both sides

    // ── mid-merge conflict, and a bisect in progress ─────────────────────────
    let conflict = testutil::merge_conflict_fixture();
    conflict.git(&["config", "user.name", "Ravi Patel"]);
    conflict.git(&["config", "user.email", "ravi@orbit.example"]);

    let bisect = testutil::Fixture::new("bisect-demo");
    bisect.git(&["config", "user.name", "Mika Sato"]);
    bisect.git(&["config", "user.email", "mika@orbit.example"]);
    for i in 0..14 {
        bisect.commit_file_at(
            "src/guidance.rs",
            &format!("pub fn heading() -> f64 {{ {} }}\n", i),
            &format!("guidance: recalibrate loop gain ({i})"),
            1_759_500_000 + i * 600,
        );
    }
    bisect.git(&["bisect", "start", "HEAD", "HEAD~12"]);
    bisect.git(&["bisect", "good", "HEAD~6"]);
    bisect.git(&["bisect", "bad", "HEAD~3"]);

    // Stable, readable paths: the nav and the top bar show these names.
    let orbit_path = relocate(&orbit, "orbit");
    let navigator_path = relocate(&conflict, "navigator");
    let guidance_path = relocate(&bisect, "guidance");
    let telemetry_wt = docs_root("repos").join("orbit-telemetry");
    testutil::git(
        &orbit_path,
        &["worktree", "add", "-q", telemetry_wt.to_str().unwrap(), "release/1.3"],
    );

    // ── the backend, pointed at a scratch store ─────────────────────────────
    let (backend, _rx) = Backend::new();
    let store = out.join("app-data");
    std::fs::remove_dir_all(&store).ok();
    std::fs::create_dir_all(&store).expect("app-data dir");
    backend.set_store(store.join("app.json"));

    let orbit_id = backend.open(orbit_path.to_str().expect("path")).expect("open orbit").id;
    let conflict_id = backend.open(navigator_path.to_str().expect("path")).expect("open navigator").id;
    let bisect_id = backend.open(guidance_path.to_str().expect("path")).expect("open guidance").id;
    let group_id = backend
        .group_add("Orbit labs", None)
        .expect("group")
        .groups
        .last()
        .expect("group")
        .id
        .clone();
    let mut ws = backend.workspace();
    let moved: Vec<ChildNode> = ws.groups[0]
        .children
        .drain(..)
        .filter(|child| match child {
            ChildNode::Group { id } => id != &group_id,
            ChildNode::Repo { .. } => true,
        })
        .collect();
    ws.groups
        .iter_mut()
        .find(|g| g.id == group_id)
        .expect("group")
        .children = moved;
    ws.groups[0].children.push(ChildNode::Group { id: group_id.clone() });
    backend.set_workspace(ws).expect("workspace");
    backend.repo_pin(&orbit_id, true).expect("pin");
    backend.repo_touch(&orbit_id);

    let orbit_graph = backend.load_graph(&orbit_id).expect("orbit graph");
    let selection = orbit_graph
        .nodes
        .iter()
        .find(|n| n.subject.starts_with("reuse burn windows"))
        .expect("selected commit");
    backend
        .save_repo_selection(
            &orbit_id,
            RepoSelection {
                commit: Some(selection.hash.clone()),
                branch: Some("main".into()),
            },
        )
        .expect("selection");

    let repos = [orbit_id.clone(), conflict_id.clone(), bisect_id.clone()];
    for cmd in [
        "load_status",
        "load_graph",
        "load_branches",
        "load_remotes",
        "load_tags",
        "load_worktrees",
        "load_stashes",
        "load_submodules",
        "load_bisect",
        "load_repo_selection",
    ] {
        let mut map = Map::new();
        for id in &repos {
            let value = match cmd {
                "load_status" => json(backend.load_status(id).expect(cmd)),
                "load_graph" => json(backend.load_graph(id).expect(cmd)),
                "load_branches" => json(backend.load_branches(id).expect(cmd)),
                "load_remotes" => json(backend.load_remotes(id).expect(cmd)),
                "load_tags" => json(backend.load_tags(id).expect(cmd)),
                "load_worktrees" => json(backend.load_worktrees(id).expect(cmd)),
                "load_stashes" => json(backend.load_stashes(id).expect(cmd)),
                "load_submodules" => json(backend.load_submodules(id).expect(cmd)),
                "load_bisect" => json(backend.load_bisect(id).expect(cmd)),
                _ => json(backend.load_repo_selection(id).expect(cmd)),
            };
            map.insert(id.clone(), value);
        }
        write_map(out, cmd, &map);
    }

    let mut commits = Map::new();
    for id in &repos {
        for node in backend.load_graph(id).expect("graph").nodes.iter().take(24) {
            commits.insert(
                format!("{id}::{}", node.hash),
                json(backend.load_commit(id, &node.hash).expect("commit")),
            );
        }
    }
    write_map(out, "load_commit", &commits);

    // Stage 15's inline diffstat column (SPEC §10.1): exactly the request the graph makes.
    let mut stats = Map::new();
    for id in &repos {
        let hashes: Vec<String> = backend
            .load_graph(id)
            .expect("graph")
            .nodes
            .iter()
            .map(|node| node.hash.clone())
            .collect();
        stats.insert(
            id.clone(),
            json(backend.load_commit_stats(id, &hashes).expect("commit stats")),
        );
    }
    write_map(out, "load_commit_stats", &stats);

    let mut diffs = Map::new();
    for path in ["src/planner.rs", "src/telemetry.rs", "notes/scratch.md", "notes/checklist.md", "f.txt"] {
        let value = match backend.load_diff(&orbit_id, path) {
            Ok(diff) => json(diff),
            Err(_) => json(backend.load_diff(&conflict_id, path).expect(path)),
        };
        diffs.insert(format!("{orbit_id}::{path}"), value);
    }
    write_map(out, "load_diff", &diffs);
    write_map(
        out,
        "load_conflict",
        &Map::from_iter([(
            format!("{conflict_id}::f.txt"),
            json(backend.load_conflict(&conflict_id, "f.txt").expect("conflict")),
        )]),
    );

    let plans = [
        ("commit", Op::Commit { message: "feat: adaptive burn windows".into(), amend: false, author: None, co_authors: vec![] }),
        ("push", Op::Push { remote: "origin".into(), branch: Some("main".into()), force: false, set_upstream: false }),
        ("pull", Op::Pull { rebase: true }),
        ("merge", Op::Merge { source: "release/1.3".into() }),
        ("reset", Op::Reset { mode: ResetMode::Hard, target: backend.load_graph(&orbit_id).expect("graph").nodes[3].hash.clone() }),
        ("worktree_remove", Op::WorktreeRemove { path: telemetry_wt.to_string_lossy().into_owned(), force: true }),
        ("stash_push", Op::StashPush { include_untracked: true }),
    ];
    let mut plan_map = Map::new();
    for (key, op) in plans {
        plan_map.insert(key.to_string(), json(backend.plan(&orbit_id, &op).expect(key)));
    }
    write_map(out, "plan_op", &plan_map);

    // Real commands, so the activity-log fixture carries the exact git invocations
    // GitGotten ran - including one git refuses, which is how the UI shows a failure.
    for op in [
        Op::RemoteFetch { remote: "origin".into(), refspec: None },
        Op::Push { remote: "origin".into(), branch: Some("release/1.3".into()), force: false, set_upstream: false },
        Op::BranchDelete { name: "main".into(), remote: None, force: false },
    ] {
        let plan = backend.plan(&orbit_id, &op).expect("op plan");
        backend.run_op(&orbit_id, &plan).await.ok();
    }

    write_json(out, "workspace_get", &json(backend.workspace()));
    write_json(out, "workspace_repos", &json(backend.workspace_repos()));
    write_json(out, "log", &json(backend.log().snapshot(Some(&orbit_id))));
    write_json(out, "build_info", &json(json!({
        "name": "GitGotten",
        "version": "0.1.0",
        "target": "aarch64-apple-darwin",
        "arch": "arm64",
    })));

    println!(
        "orbit {} conflict {} bisect {}",
        backend.load_graph(&orbit_id).expect("g").nodes.len(),
        backend.load_graph(&conflict_id).expect("g").nodes.len(),
        backend.load_graph(&bisect_id).expect("g").nodes.len(),
    );
}
