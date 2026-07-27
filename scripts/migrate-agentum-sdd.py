#!/usr/bin/env python3
"""One-time, deterministic migration of Agentum's own legacy SDD material.

Preview is read-only. Apply first copies and verifies every source in an
external recovery archive, atomically publishes native .agentum artifacts and
the hash inventory, then removes only the explicitly retired roots and demo.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import sqlite3
import stat
import subprocess
import sys
import tempfile


REPORT_PATH = Path("docs/migrations/agentum-sdd-v2-inventory.json")
JOURNAL_PATH = Path(".agentum-migration-journal.json")
LEGACY_DEMO_DIRECTORY = Path("examples/harness-demo")
LEGACY_DEMO_FILES = (
    Path("examples/harness-demo/.harness/AGENTS.md"),
    Path("examples/harness-demo/.harness/feature_list.json"),
    Path("examples/harness-demo/.harness/handoff.md"),
    Path("examples/harness-demo/.harness/init.sh"),
    Path("examples/harness-demo/.harness/verify.sh"),
    Path("examples/harness-demo/README.md"),
)
REPLACEMENT_DEMO_FILES = (
    Path("examples/sdd-demo/README.md"),
    Path("examples/sdd-demo/package.json"),
    Path("examples/sdd-demo/src/session-store.js"),
    Path("examples/sdd-demo/test/session-store.test.js"),
)
LEGACY_DIRECTORIES = (Path("ai"), Path(".agentum-harness"), LEGACY_DEMO_DIRECTORY)
LEGACY_ROOT_FILES = (Path("spec.md"), Path("architecture.md"), Path("execution-plan.json"))
LEGACY_ROOTS = (*LEGACY_DIRECTORIES, *LEGACY_ROOT_FILES)
CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def explicit_repo_root(requested: Path) -> Path:
    if not requested.is_absolute():
        raise ValueError("--repo-root must be an absolute path")
    requested = Path(os.path.abspath(requested))
    result = subprocess.run(
        ["git", "-C", requested, "rev-parse", "--show-toplevel"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    discovered = Path(result.stdout.strip()).resolve()
    if discovered != requested or requested.resolve(strict=True) != requested:
        raise ValueError("--repo-root must name the exact unsymlinked Git repository root")
    return requested


def require_same_repo_root(root: Path) -> None:
    result = subprocess.run(
        ["git", "-C", root, "rev-parse", "--show-toplevel"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    if Path(result.stdout.strip()).resolve() != root:
        raise ValueError("repository root changed during migration")


def checked_relative_path(root: Path, relative: Path) -> Path:
    if relative.is_absolute() or not relative.parts or any(part in {"", ".", ".."} for part in relative.parts):
        raise ValueError(f"unsafe legacy path: {relative}")
    in_legacy_directory = any(
        relative.parts[: len(directory.parts)] == directory.parts
        for directory in LEGACY_DIRECTORIES
    )
    if relative not in LEGACY_ROOT_FILES and not in_legacy_directory:
        raise ValueError(f"path is outside a retired root: {relative}")
    candidate = root.joinpath(relative)
    lexical = Path(os.path.abspath(candidate))
    try:
        lexical.relative_to(root)
    except ValueError as error:
        raise ValueError(f"legacy path escapes repository: {relative}") from error
    if lexical.resolve(strict=False) != lexical:
        raise ValueError(f"symlink or junction in legacy path: {relative}")
    return lexical


def read_regular_no_follow(path: Path) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError(f"source is not a regular file: {path}")
        chunks: list[bytes] = []
        while chunk := os.read(descriptor, 1024 * 1024):
            chunks.append(chunk)
        return b"".join(chunks)
    finally:
        os.close(descriptor)


def is_legacy_demo_source(relative: Path) -> bool:
    return relative in LEGACY_DEMO_FILES


def read_tracked_index_source(root: Path, relative: Path) -> bytes:
    metadata = subprocess.run(
        ["git", "ls-files", "-s", "--", relative.as_posix()],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout.strip().splitlines()
    if len(metadata) != 1:
        raise ValueError(f"deleted legacy demo source is not uniquely tracked: {relative}")
    fields = metadata[0].split(maxsplit=3)
    if len(fields) != 4 or fields[0] not in {"100644", "100755"} or fields[2] != "0":
        raise ValueError(f"deleted legacy demo source has an unsafe index entry: {relative}")
    return subprocess.run(
        ["git", "show", f":{relative.as_posix()}"],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
    ).stdout


def read_deleted_demo_from_index(root: Path, relative: Path) -> bytes:
    if not is_legacy_demo_source(relative):
        raise FileNotFoundError(relative)
    return read_tracked_index_source(root, relative)


def read_source(root: Path, relative: Path, expected_hash: str | None = None) -> bytes:
    path = checked_relative_path(root, relative)
    try:
        data = read_regular_no_follow(path)
    except FileNotFoundError:
        data = read_deleted_demo_from_index(root, relative)
    digest = sha256(data)
    if expected_hash is not None and digest != expected_hash:
        raise ValueError(f"legacy source changed during migration: {relative}")
    return data


def tracked_legacy_files(root: Path) -> list[Path]:
    result = subprocess.run(
        [
            "git",
            "ls-files",
            "-z",
            "--",
            "ai",
            ".agentum-harness",
            "examples/harness-demo",
            "spec.md",
            "architecture.md",
            "execution-plan.json",
        ],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
    )
    sources = [Path(os.fsdecode(value)) for value in result.stdout.split(b"\0") if value]
    for source in sources:
        checked_relative_path(root, source)
    return sources


def require_demo_source_state(root: Path) -> bool:
    """Return true when the retired demo is already replaced and deleted.

    The current rewrite removed the six tracked demo files before this
    migration existed. That exact all-deleted state is safe to account from
    Git's index only when the neutral replacement fixture is fully present.
    Mixed, modified, renamed, or untracked legacy-demo states remain blocked.
    """
    result = subprocess.run(
        [
            "git",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            LEGACY_DEMO_DIRECTORY.as_posix(),
        ],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
    )
    records = [value for value in result.stdout.split(b"\0") if value]
    if not records:
        return False
    expected = {f" D {path.as_posix()}".encode() for path in LEGACY_DEMO_FILES}
    if set(records) != expected:
        raise ValueError("legacy demo is dirty in a state other than its exact replacement deletion")
    for relative in REPLACEMENT_DEMO_FILES:
        candidate = root / relative
        if candidate.is_symlink() or not candidate.is_file():
            raise ValueError(f"legacy demo deletion lacks the neutral replacement fixture: {relative}")
    return True


def validate_tracked_demo_set(root: Path, sources: list[Path]) -> None:
    tracked_demo = {path for path in sources if is_legacy_demo_source(path)}
    unexpected = {
        path
        for path in sources
        if path.parts[: len(LEGACY_DEMO_DIRECTORY.parts)] == LEGACY_DEMO_DIRECTORY.parts
        and path not in LEGACY_DEMO_FILES
    }
    if unexpected or (tracked_demo and tracked_demo != set(LEGACY_DEMO_FILES)):
        raise ValueError("tracked legacy demo does not match the six-file migration contract")
    if tracked_demo and any(not (root / path).exists() for path in tracked_demo):
        require_demo_source_state(root)


def require_clean_legacy_roots(root: Path) -> None:
    result = subprocess.run(
        [
            "git",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            "ai",
            ".agentum-harness",
            "spec.md",
            "architecture.md",
            "execution-plan.json",
        ],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
    )
    if result.stdout:
        raise ValueError(
            "legacy SDD roots are dirty; commit, remove, or explicitly snapshot those changes first"
        )


def require_only_inventoried_sources(
    root: Path, sources: list[Path], owned_missing: set[str] | None = None
) -> None:
    expected = {path.as_posix() for path in sources}
    observed: set[str] = set()
    for relative in LEGACY_DIRECTORIES:
        directory = root / relative
        if not directory.exists() and not directory.is_symlink():
            continue
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f"legacy root is not a real directory: {relative}")
        for current, directories, files in os.walk(directory, followlinks=False):
            current_path = Path(current)
            for name in directories:
                child = current_path / name
                metadata = child.lstat()
                if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
                    raise ValueError(f"symlink or special directory in legacy root: {child}")
            for name in files:
                child = current_path / name
                metadata = child.lstat()
                if not stat.S_ISREG(metadata.st_mode):
                    raise ValueError(f"symlink or special file in legacy root: {child}")
                observed.add(child.relative_to(root).as_posix())
    for relative in LEGACY_ROOT_FILES:
        root_artifact = root / relative
        if root_artifact.exists() or root_artifact.is_symlink():
            metadata = root_artifact.lstat()
            if not stat.S_ISREG(metadata.st_mode):
                raise ValueError(f"root {relative} is a symlink or special file")
            observed.add(relative.as_posix())
    extras = sorted(observed - expected)
    missing = sorted(expected - observed)
    if owned_missing is not None:
        missing = sorted(set(missing) - owned_missing)
    elif missing and require_demo_source_state(root):
        allowed_missing = {path.as_posix() for path in LEGACY_DEMO_FILES}
        missing = sorted(set(missing) - allowed_missing)
    if extras or missing:
        details = []
        if extras:
            details.append(f"untracked: {', '.join(extras[:5])}")
        if missing:
            details.append(f"missing: {', '.join(missing[:5])}")
        raise ValueError("legacy roots differ from the inventory (" + "; ".join(details) + ")")


def default_database_path() -> Path:
    override = os.environ.get("AGENTUM_HOME", "").strip()
    if override:
        return Path(override).expanduser() / "data" / "db.sqlite"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "agentum" / "db.sqlite"
    if os.name == "nt":
        # Keep this in lockstep with `directories::ProjectDirs::data_dir()` in
        # agentum-store. On Windows that is the roaming application-data root;
        # `LOCALAPPDATA` is `data_local_dir()` and can hold a different database.
        roaming = os.environ.get("APPDATA")
        base = Path(roaming) if roaming else Path.home() / "AppData" / "Roaming"
        return base / "agentum" / "data" / "db.sqlite"
    base = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local" / "share")))
    return base / "agentum" / "db.sqlite"


def require_no_active_v1_run(root: Path) -> None:
    feature_list = root / ".agentum-harness" / "feature_list.json"
    if feature_list.is_file():
        payload = json.loads(read_source(root, Path(".agentum-harness/feature_list.json")))
        active_states = {"coding", "verifying", "ready_to_test", "awaiting_confirm", "blocked"}
        active = [
            str(feature.get("id", "unknown"))
            for feature in payload.get("features", [])
            if str(feature.get("state", "")).lower() in active_states
        ]
        if active:
            raise ValueError(f"active v1 feature(s) block cutover: {', '.join(active)}")

    database = default_database_path()
    if not database.is_file() or database.is_symlink():
        return
    connection = sqlite3.connect(f"file:{database.as_posix()}?mode=ro", uri=True)
    try:
        table = connection.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='harness_orchestrated_runs'"
        ).fetchone()
        if not table:
            return
        terminal = ("completed", "stopped", "recovery_quarantined", "blocked_remote_mode")
        columns = {
            str(row[1])
            for row in connection.execute("PRAGMA table_info(harness_orchestrated_runs)")
        }
        scope_expression = "COALESCE(scope_path, '')" if "scope_path" in columns else "''"
        rows = connection.execute(
            f"SELECT run_id, workdir, {scope_expression}, status FROM harness_orchestrated_runs"
        ).fetchall()
        for run_id, workdir, scope_path, status in rows:
            if status in terminal:
                continue
            for candidate in (workdir, scope_path):
                if not candidate:
                    continue
                try:
                    resolved = Path(candidate).resolve()
                    if resolved == root or root in resolved.parents or resolved in root.parents:
                        raise ValueError(f"active v1 run blocks cutover: {run_id} ({status})")
                except OSError:
                    continue
    finally:
        connection.close()


def encode_ulid(seed: bytes) -> str:
    value = bytearray(hashlib.sha256(seed).digest()[:16])
    # A 128-bit ULID encoded in 26 characters must start between 0 and 7.
    value[0] &= 0x3F
    number = int.from_bytes(value, "big")
    encoded = []
    for _ in range(26):
        encoded.append(CROCKFORD[number & 31])
        number >>= 5
    return "".join(reversed(encoded))


def slugify(value: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")[:48].strip("-")
    return slug or "spec"


def title_from(path: Path, content: bytes) -> str:
    text = content.decode("utf-8", errors="replace")
    for line in text.splitlines():
        match = re.match(r"^#\s+(.+?)\s*$", line)
        if match:
            title = re.sub(r"^(?:Spec\s+[^—:-]+\s*[—:-]\s*)", "", match.group(1))
            return title.replace("\r", " ").replace("\n", " ").strip()[:160]
    return path.parent.name.replace("-", " ").strip().title()[:160] or "Imported specification"


def is_spec_source(path: Path) -> bool:
    value = path.as_posix()
    return value == "spec.md" or value.endswith("/spec.md")


def spec_family(path: Path) -> Path | None:
    if path in LEGACY_ROOT_FILES:
        return Path(".")
    parts = path.parts
    if len(parts) >= 4 and parts[0] == "ai" and parts[1] == "specs":
        return Path(*parts[:3])
    if len(parts) >= 4 and parts[0] == ".agentum-harness" and parts[1] == "specs":
        return Path(*parts[:3])
    return None


def valid_relative_scope(value: str) -> bool:
    path = Path(value)
    return bool(value) and not path.is_absolute() and all(
        part not in {"", ".", ".."} for part in path.parts
    )


def acceptance_references(values: list[object]) -> list[str]:
    references: list[str] = []
    for value in values:
        if not isinstance(value, str):
            continue
        for match in re.finditer(r"\bAC[- ]?(\d+)\b", value, re.IGNORECASE):
            reference = f"AC-{int(match.group(1)):03d}"
            if reference not in references:
                references.append(reference)
    return references


def command_spec(command: object) -> dict[str, object] | None:
    if not isinstance(command, str) or not command.strip():
        return None
    try:
        tokens = shlex.split(command, posix=True)
    except ValueError:
        return None
    cwd = "."
    if len(tokens) >= 4 and tokens[0] == "cd" and tokens[2] == "&&":
        cwd = tokens[1]
        tokens = tokens[3:]
    shell_tokens = {"&&", "||", ";", "|", ">", ">>", "<", "2>", "2>&1"}
    if not tokens or any(token in shell_tokens for token in tokens) or not valid_relative_scope(cwd):
        return None
    return {
        "program": tokens[0],
        "args": tokens[1:],
        "cwd": cwd,
        "envAllowlist": [],
        "timeoutMs": 900_000,
        "outputLimit": 1_048_576,
    }


def markdown_plan(content: str, spec_id: str) -> tuple[str | None, str]:
    checkbox = re.compile(r"^\s*[-*]\s+\[[ xX~]\]\s+(.+?)\s*$", re.MULTILINE)
    objectives = [re.sub(r"\s+", " ", value).strip() for value in checkbox.findall(content)]
    normalization = "checklist entries converted to conservative typed tasks"
    if not objectives:
        heading = re.compile(r"^##\s+(.+?)\s*$", re.MULTILINE)
        objectives = [
            f"Carry out the migrated plan section: {re.sub(r'\s+', ' ', value).strip()}"
            for value in heading.findall(content)
        ]
        normalization = "level-two sections converted to conservative typed tasks; prose details remain in the recovery archive"
    if not objectives:
        return None, "tasks Markdown has no checklist entries or level-two task sections"
    tasks = []
    for index, objective in enumerate(objectives, start=1):
        tasks.append(
            {
                "id": f"T-{index:03d}",
                "objective": objective,
                "dependencies": [],
                "readScopes": [],
                "writeScopes": [],
                "acceptanceCriteria": acceptance_references([objective]),
                "verification": [],
                "risk": "unknown",
                # The old Markdown does not carry enough information to prove
                # lease compatibility. Preserve intent but schedule serially.
                "parallelSafe": False,
            }
        )
    plan = {"schemaVersion": 1, "specId": spec_id, "specRevision": 1, "tasks": tasks}
    return json.dumps(plan, indent=2) + "\n", normalization


def execution_plan(content: str, spec_id: str) -> tuple[str | None, str]:
    try:
        legacy = json.loads(content)
    except json.JSONDecodeError as error:
        return None, f"execution plan is malformed JSON: {error.msg}"
    if not isinstance(legacy, dict) or legacy.get("version") != 1 or not isinstance(legacy.get("tasks"), list):
        return None, "execution plan does not match the supported v1 structure"

    diagnostics: list[str] = []
    tasks: list[dict[str, object]] = []
    known_ids: set[str] = set()
    raw_tasks = legacy["tasks"]
    for index, raw in enumerate(raw_tasks, start=1):
        if not isinstance(raw, dict):
            return None, f"execution plan task {index} is not an object"
        task_id = str(raw.get("id", "")).strip()
        objective = str(raw.get("objective", "")).strip()
        if not task_id or task_id in known_ids or not objective:
            return None, f"execution plan task {index} has an invalid id or objective"
        known_ids.add(task_id)

        read_scopes: list[str] = []
        for item in raw.get("read_only", []):
            value = item.get("path") if isinstance(item, dict) else item
            if isinstance(value, str) and valid_relative_scope(value):
                read_scopes.append(value)
            elif value is not None:
                diagnostics.append(f"{task_id}: unsafe or malformed read scope omitted")
        write_scopes: list[str] = []
        for value in [*raw.get("writable_files", []), *raw.get("allowed_create_dirs", [])]:
            if isinstance(value, str) and valid_relative_scope(value):
                write_scopes.append(value)
            else:
                diagnostics.append(f"{task_id}: unsafe or malformed write scope omitted")
        verification: list[dict[str, object]] = []
        gate = raw.get("targeted_gate")
        if isinstance(gate, dict) and gate.get("command"):
            normalized = command_spec(gate.get("command"))
            if normalized is None:
                diagnostics.append(f"{task_id}: shell-dependent targeted gate omitted")
            else:
                verification.append(normalized)
        tasks.append(
            {
                "id": task_id,
                "objective": objective,
                "dependencies": [
                    value for value in raw.get("dependencies", []) if isinstance(value, str)
                ],
                "readScopes": list(dict.fromkeys(read_scopes)),
                "writeScopes": list(dict.fromkeys(write_scopes)),
                "acceptanceCriteria": acceptance_references(raw.get("acceptance_checks", [])),
                "verification": verification,
                "risk": "medium" if write_scopes else "low",
                "parallelSafe": False,
            }
        )

    for task in tasks:
        unknown = [value for value in task["dependencies"] if value not in known_ids]
        if unknown:
            return None, f"execution plan task {task['id']} has unknown dependencies"

    final_ids: list[str] = []
    for index, gate in enumerate(legacy.get("final_gates", []), start=1):
        if not isinstance(gate, dict):
            diagnostics.append(f"final gate {index}: malformed gate omitted")
            continue
        normalized = command_spec(gate.get("command"))
        if normalized is None:
            diagnostics.append(f"final gate {index}: shell-dependent command omitted")
            continue
        task_id = f"MIG-FINAL-{index:03d}"
        final_ids.append(task_id)
        tasks.append(
            {
                "id": task_id,
                "objective": f"Run migrated final verification gate {index}",
                "dependencies": sorted(known_ids),
                "readScopes": [],
                "writeScopes": [],
                "acceptanceCriteria": acceptance_references(gate.get("acceptance_checks", [])),
                "verification": [normalized],
                "risk": "low",
                "parallelSafe": False,
            }
        )
    plan = {"schemaVersion": 1, "specId": spec_id, "specRevision": 1, "tasks": tasks}
    summary = "v1 JSON intent converted to typed tasks and CommandSpec verification"
    if diagnostics:
        summary += "; " + "; ".join(diagnostics)
    return json.dumps(plan, indent=2) + "\n", summary


def generated_legacy_surface(path: Path) -> bool:
    value = path.as_posix()
    if value.startswith("examples/harness-demo/.harness/"):
        return True
    if value.startswith(".agentum-harness/") and "/specs/" not in value:
        return True
    return value.startswith(
        (
            "ai/contracts/",
            "ai/orchestration/",
            "ai/roles/",
            "ai/skills/",
        )
    )


def native_spec(
    spec_id: str,
    title: str,
    source: str,
    source_hash: str,
    source_content: str,
) -> str:
    quoted_source = "\n".join(f"> {line}" if line else ">" for line in source_content.rstrip().splitlines())
    return f"""---
schema: 1
id: {spec_id}
revision: 1
title: {title}
source: legacy-import:{source}@sha256:{source_hash}
---

# {title}

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

{quoted_source}
"""


def validate_inventory(
    report: dict[str, object], imported: dict[str, str], expected_sources: set[str]
) -> None:
    entries = report.get("sources")
    if not isinstance(entries, list) or report.get("sourceCount") != len(entries):
        raise ValueError("migration inventory source count is inconsistent")
    paths = [str(entry.get("path", "")) for entry in entries if isinstance(entry, dict)]
    if len(paths) != len(entries) or len(set(paths)) != len(paths) or set(paths) != expected_sources:
        raise ValueError("migration inventory does not account for every source exactly once")
    if any(
        not re.fullmatch(r"[0-9a-f]{64}", str(entry.get("sha256", "")))
        for entry in entries
        if isinstance(entry, dict)
    ):
        raise ValueError("migration inventory contains an invalid source hash")
    imported_destinations = {
        str(entry["destination"])
        for entry in entries
        if isinstance(entry, dict) and entry.get("disposition") == "imported revision"
    }
    if imported_destinations != set(imported):
        raise ValueError("migration artifact associations do not match imported files")
    associations = report.get("artifactAssociations")
    if not isinstance(associations, list):
        raise ValueError("migration inventory has no artifact associations")
    associated_destinations: set[str] = set()
    for association in associations:
        if not isinstance(association, dict) or not association.get("specId"):
            raise ValueError("migration inventory has a malformed artifact association")
        artifacts = association.get("artifacts")
        if not isinstance(artifacts, list) or not any(
            isinstance(artifact, dict) and artifact.get("kind") == "spec" for artifact in artifacts
        ):
            raise ValueError("every imported SPC must be associated with spec.md")
        for artifact in artifacts:
            if not isinstance(artifact, dict) or artifact.get("destination") not in imported:
                raise ValueError("artifact association points outside the imported set")
            associated_destinations.add(str(artifact["destination"]))
    if associated_destinations != set(imported):
        raise ValueError("not every imported artifact is associated with an SPC")


def build_inventory(
    root: Path, sources: list[Path], restricted_matches: set[str] | None = None
) -> tuple[dict, dict[str, str]]:
    restricted_matches = restricted_matches or set()
    file_data = {path.as_posix(): read_source(root, path) for path in sources}
    file_hashes = {path: sha256(data) for path, data in file_data.items()}
    archive_id = sha256(
        "".join(f"{path}\0{file_hashes[path]}\n" for path in sorted(file_hashes)).encode()
    )
    artifact_set_id = encode_ulid(f"artifact-set\0{archive_id}".encode())

    restricted_families = {
        family
        for source in file_data
        if source in restricted_matches
        and (family := spec_family(Path(source))) is not None
    }
    seen_spec_hashes: dict[str, dict[str, str]] = {}
    family_records: dict[Path, dict[str, str]] = {}
    imported: dict[str, str] = {}
    destination_sources: dict[str, str] = {}
    association_by_id: dict[str, dict[str, object]] = {}
    spec_entries: dict[str, dict[str, object]] = {}

    for source in sorted(file_data):
        path = Path(source)
        family = spec_family(path)
        if not is_spec_source(path) or family is None or family in restricted_families:
            continue
        try:
            source_content = file_data[source].decode("utf-8")
        except UnicodeDecodeError:
            continue
        if not source_content.strip():
            continue
        digest = file_hashes[source]
        existing = seen_spec_hashes.get(digest)
        if existing is not None:
            family_records[family] = existing
            spec_entries[source] = {
                "disposition": "exact duplicate",
                "duplicateOf": existing["source"],
                "destination": existing["destination"],
                "specId": existing["specId"],
                "artifactKind": "spec",
            }
            continue
        title = title_from(path, file_data[source])
        ulid = encode_ulid(f"spec\0{source}\0{digest}".encode())
        spec_id = f"SPC-{ulid}"
        directory = f".agentum/specs/spc-{ulid.lower()}-{slugify(title)}"
        destination = f"{directory}/spec.md"
        record = {
            "source": source,
            "specId": spec_id,
            "directory": directory,
            "destination": destination,
        }
        seen_spec_hashes[digest] = record
        family_records[family] = record
        spec_entries[source] = {
            "disposition": "imported revision",
            "destination": destination,
            "specId": spec_id,
            "artifactKind": "spec",
            "normalization": "Agentum frontmatter and migration criteria prepended; original UTF-8 source body blockquoted",
        }
        imported[destination] = native_spec(spec_id, title, source, digest, source_content)
        destination_sources[destination] = source
        association_by_id[spec_id] = {
            "specId": spec_id,
            "specSource": source,
            "specDestination": destination,
            "artifacts": [
                {
                    "kind": "spec",
                    "source": source,
                    "sha256": digest,
                    "destination": destination,
                }
            ],
        }

    entries: list[dict] = []
    for source in sorted(file_data):
        path = Path(source)
        digest = file_hashes[source]
        entry: dict[str, object] = {"path": source, "sha256": digest}
        family = spec_family(path)
        restricted = source in restricted_matches
        if restricted or family in restricted_families:
            entry["disposition"] = "externally archived recovery material"
            entry["diagnostic"] = "source family matched the externally supplied restricted-content policy"
        elif is_spec_source(path):
            if source in spec_entries:
                entry.update(spec_entries[source])
            else:
                entry["disposition"] = "historical-only"
                entry["diagnostic"] = "specification is empty or not valid UTF-8 Markdown"
        elif family in family_records and path.parent == family:
            record = family_records[family]
            artifact_kind: str | None = None
            destination_name: str | None = None
            content: str | None = None
            normalization: str | None = None
            if path.name in {"architecture.md", "design.md"}:
                artifact_kind, destination_name = "design", "design.md"
                try:
                    decoded = file_data[source].decode("utf-8")
                    content = decoded.rstrip() + "\n" if decoded.strip() else None
                    normalization = "legacy architecture/design Markdown mapped to design.md"
                except UnicodeDecodeError:
                    content = None
            elif path.name == "tasks.md":
                artifact_kind, destination_name = "plan", "plan.json"
                try:
                    content, normalization = markdown_plan(
                        file_data[source].decode("utf-8"), record["specId"]
                    )
                except UnicodeDecodeError:
                    content, normalization = None, "tasks Markdown is not valid UTF-8"
            elif path == Path("execution-plan.json"):
                artifact_kind, destination_name = "plan", "plan.json"
                try:
                    content, normalization = execution_plan(
                        file_data[source].decode("utf-8"), record["specId"]
                    )
                except UnicodeDecodeError:
                    content, normalization = None, "execution plan is not valid UTF-8"
            elif path.name in {"review.md", "decisions.md"}:
                artifact_kind, destination_name = path.stem, path.name
                try:
                    decoded = file_data[source].decode("utf-8")
                    content = decoded.rstrip() + "\n" if decoded.strip() else None
                    normalization = f"legacy {path.name} associated with its stable SPC"
                except UnicodeDecodeError:
                    content = None

            if artifact_kind and destination_name and content:
                destination = f"{record['directory']}/{destination_name}"
                if destination in imported:
                    if imported[destination].encode() == content.encode():
                        entry.update(
                            {
                                "disposition": "exact duplicate",
                                "duplicateOf": destination_sources[destination],
                                "destination": destination,
                                "specId": record["specId"],
                                "artifactKind": artifact_kind,
                            }
                        )
                    else:
                        entry.update(
                            {
                                "disposition": "conflict",
                                "conflictsWith": destination_sources[destination],
                                "specId": record["specId"],
                                "artifactKind": artifact_kind,
                                "diagnostic": f"multiple legacy sources map to {destination_name}; no overwrite was attempted",
                            }
                        )
                else:
                    entry.update(
                        {
                            "disposition": "imported revision",
                            "destination": destination,
                            "specId": record["specId"],
                            "artifactKind": artifact_kind,
                            "normalization": normalization,
                        }
                    )
                    imported[destination] = content
                    destination_sources[destination] = source
                    association_by_id[record["specId"]]["artifacts"].append(
                        {
                            "kind": artifact_kind,
                            "source": source,
                            "sha256": digest,
                            "destination": destination,
                        }
                    )
            elif artifact_kind:
                entry["disposition"] = "historical-only"
                entry["specId"] = record["specId"]
                entry["artifactKind"] = artifact_kind
                entry["diagnostic"] = normalization or "artifact is empty or not valid UTF-8"
            elif generated_legacy_surface(path):
                entry["disposition"] = "intentionally ignored generated data"
            else:
                entry["disposition"] = "historical-only"
        elif generated_legacy_surface(path):
            entry["disposition"] = "intentionally ignored generated data"
        else:
            entry["disposition"] = "historical-only"
        entry["archiveRelativePath"] = source
        entries.append(entry)

    counts: dict[str, int] = {}
    for entry in entries:
        disposition = str(entry["disposition"])
        counts[disposition] = counts.get(disposition, 0) + 1
    report = {
        "schemaVersion": 1,
        "migration": "agentum-sdd-v2-hard-cutover",
        "archiveId": archive_id,
        "artifactSetId": artifact_set_id,
        "sourceCount": len(entries),
        "dispositionCounts": dict(sorted(counts.items())),
        "artifactAssociations": sorted(association_by_id.values(), key=lambda value: str(value["specId"])),
        "sources": entries,
    }
    validate_inventory(report, imported, set(file_data))
    return report, imported


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=False) + "\n", encoding="utf-8")


def require_unsymlinked_absolute(path: Path, label: str) -> Path:
    absolute = Path(os.path.abspath(path))
    if absolute.resolve(strict=False) != absolute:
        raise ValueError(f"{label} contains a symlink or junction: {path}")
    return absolute


def restricted_pattern_file(root: Path, path: Path) -> tuple[Path, list[str]]:
    if not path.is_absolute():
        raise ValueError("restricted pattern file must be an absolute path")
    absolute = require_unsymlinked_absolute(path, "restricted pattern file")
    if absolute == root or root in absolute.parents:
        raise ValueError("restricted pattern file must remain outside the repository")
    data = read_regular_no_follow(absolute)
    try:
        lines = data.decode("utf-8").splitlines()
    except UnicodeDecodeError as error:
        raise ValueError("restricted pattern file must be UTF-8") from error
    active = [line for line in lines if line and not line.startswith("#")]
    if not active:
        raise ValueError("restricted pattern file contains no active patterns")
    return absolute, active


def scan_restricted_matches(
    root: Path, pattern_path: Path, candidates: list[Path]
) -> set[str]:
    _absolute, patterns = restricted_pattern_file(root, pattern_path)
    for candidate in candidates:
        if candidate.is_absolute() or not candidate.parts or any(
            part in {"", ".", ".."} for part in candidate.parts
        ):
            raise ValueError(f"unsafe restricted-content scan path: {candidate}")
    descriptor, filtered_name = tempfile.mkstemp(prefix="agentum-restricted-patterns-")
    filtered = Path(filtered_name)
    try:
        os.fchmod(descriptor, 0o600)
        payload = ("\n".join(patterns) + "\n").encode()
        view = memoryview(payload)
        while view:
            written = os.write(descriptor, view)
            view = view[written:]
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = -1
        present_candidates = [
            candidate for candidate in candidates if (root / candidate).exists()
        ]
        matches: set[str] = set()
        if present_candidates:
            result = subprocess.run(
                [
                    "rg",
                    "--files-with-matches",
                    "--null",
                    "--hidden",
                    "--no-messages",
                    "--file",
                    str(filtered),
                    "--",
                    *(candidate.as_posix() for candidate in present_candidates),
                ],
                cwd=root,
                stdout=subprocess.PIPE,
            )
            if result.returncode not in {0, 1}:
                raise ValueError("restricted-content scan failed")
            matches = {
                os.fsdecode(value).removeprefix("./")
                for value in result.stdout.split(b"\0")
                if value
            }
        for candidate in candidates:
            path = root / candidate
            if path.exists() or path.is_symlink():
                continue
            data = read_source(root, candidate)
            probe = subprocess.run(
                ["rg", "--quiet", "--file", str(filtered), "-"],
                input=data,
            )
            if probe.returncode == 0:
                matches.add(candidate.as_posix())
            elif probe.returncode != 1:
                raise ValueError("restricted-content scan failed for deleted legacy demo source")
        return matches
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        try:
            filtered.unlink()
        except FileNotFoundError:
            pass


def validate_native_artifacts(root: Path, artifact_root: Path) -> None:
    checker = root / "scripts" / "check-agentum-artifacts.py"
    # Unit-test repositories do not carry the repository's checker. Import it
    # from the migration script's source checkout in that case.
    if not checker.is_file():
        checker = Path(__file__).resolve().with_name("check-agentum-artifacts.py")
    result = subprocess.run(
        [sys.executable, checker, artifact_root],
        cwd=root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if result.returncode != 0:
        diagnostic = result.stderr.strip() or result.stdout.strip() or "unknown validation error"
        raise ValueError(f"generated .agentum artifacts failed validation: {diagnostic}")


def write_exclusive_regular(path: Path, data: bytes) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags, 0o600)
    try:
        view = memoryview(data)
        while view:
            written = os.write(descriptor, view)
            view = view[written:]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def ensure_private_directory(path: Path, *, parents: bool = False) -> None:
    """Create an archive directory and keep it private to the current user.

    Recovery archives can contain historical restricted material. File mode
    0600 is insufficient when world-searchable parent directories disclose the
    archived source layout, so every directory owned by the archive is 0700.
    """
    path.mkdir(mode=0o700, parents=parents, exist_ok=True)
    require_unsymlinked_absolute(path, "private archive directory")
    if os.name != "nt":
        os.chmod(path, 0o700, follow_symlinks=False)
        if path.stat().st_mode & 0o077:
            raise ValueError(f"archive directory is not private: {path}")


def sync_directory(path: Path) -> None:
    if os.name == "nt":
        return
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def ensure_external_archive(root: Path, archive_root: Path, report: dict) -> Path:
    archive_root = require_unsymlinked_absolute(archive_root, "archive directory")
    if archive_root == root or root in archive_root.parents:
        raise ValueError("archive directory must be outside the repository")
    archive = archive_root / str(report["archiveId"])
    ensure_private_directory(archive, parents=True)
    for entry in report["sources"]:
        relative = Path(str(entry["path"]))
        destination = archive / relative
        current = archive
        for component in relative.parent.parts:
            current = current / component
            ensure_private_directory(current)
        require_unsymlinked_absolute(destination.parent, "archive parent")
        source_data = read_source(root, relative, str(entry["sha256"]))
        if destination.exists() or destination.is_symlink():
            archived_data = read_regular_no_follow(destination)
        else:
            write_exclusive_regular(destination, source_data)
            archived_data = read_regular_no_follow(destination)
        if sha256(archived_data) != entry["sha256"]:
            raise ValueError(f"archive verification failed for {relative}")
    inventory_path = archive / "inventory.json"
    inventory_data = (json.dumps(report, indent=2, sort_keys=False) + "\n").encode()
    if inventory_path.exists() or inventory_path.is_symlink():
        if read_regular_no_follow(inventory_path) != inventory_data:
            raise ValueError("existing archive inventory does not match this preview")
    else:
        write_exclusive_regular(inventory_path, inventory_data)
    os.sync()
    return archive


def encoded_json(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=False) + "\n").encode()


def manifest_bytes(report: dict) -> bytes:
    return (
        json.dumps(
            {
                "format": "agentum-sdd",
                "schemaVersion": 1,
                "artifactSetId": report["artifactSetId"],
            },
            indent=2,
        )
        + "\n"
    ).encode()


def immutable_checkpoint(report: dict, imported: dict[str, str], pattern_hash: str) -> dict:
    artifact_hashes = {
        "manifest.json": sha256(manifest_bytes(report)),
        **{
            Path(relative).relative_to(".agentum").as_posix(): sha256(content.encode())
            for relative, content in sorted(imported.items())
        },
    }
    return {
        "schemaVersion": 1,
        "migration": "agentum-sdd-v2-hard-cutover",
        "archiveId": report["archiveId"],
        "artifactSetId": report["artifactSetId"],
        "reportHash": sha256(encoded_json(report)),
        "restrictedPatternHash": pattern_hash,
        "sourceHashes": {
            str(entry["path"]): str(entry["sha256"]) for entry in report["sources"]
        },
        "artifactHashes": artifact_hashes,
    }


def ensure_archive_checkpoint(archive: Path, checkpoint: dict) -> None:
    destination = archive / "checkpoint.json"
    payload = encoded_json(checkpoint)
    if destination.exists() or destination.is_symlink():
        if read_regular_no_follow(destination) != payload:
            raise ValueError("existing migration checkpoint does not match this run")
        return
    write_exclusive_regular(destination, payload)
    sync_directory(archive)


def create_journal(root: Path, checkpoint: dict) -> dict:
    journal = {**checkpoint, "stage": "archived", "deletedCount": 0}
    destination = root / JOURNAL_PATH
    if destination.exists() or destination.is_symlink():
        raise ValueError("an incomplete migration journal already exists")
    write_exclusive_regular(destination, encoded_json(journal))
    sync_directory(root)
    return journal


def write_journal(root: Path, journal: dict, stage: str, deleted_count: int) -> dict:
    destination = root / JOURNAL_PATH
    if destination.is_symlink() or not destination.is_file():
        raise ValueError("migration journal disappeared or became unsafe")
    updated = {**journal, "stage": stage, "deletedCount": deleted_count}
    temporary = destination.with_suffix(f".tmp-{os.getpid()}")
    write_exclusive_regular(temporary, encoded_json(updated))
    os.replace(temporary, destination)
    sync_directory(root)
    return updated


def current_pattern_hash(root: Path, pattern_path: Path) -> str:
    absolute, _patterns = restricted_pattern_file(root, pattern_path)
    return sha256(read_regular_no_follow(absolute))


def validate_checkpoint_shape(checkpoint: dict, report: dict) -> None:
    required = {
        "schemaVersion",
        "migration",
        "archiveId",
        "artifactSetId",
        "reportHash",
        "restrictedPatternHash",
        "sourceHashes",
        "artifactHashes",
    }
    if set(checkpoint) != required:
        raise ValueError("migration checkpoint has unexpected fields")
    if checkpoint.get("schemaVersion") != 1 or checkpoint.get("migration") != report.get("migration"):
        raise ValueError("migration checkpoint schema is unsupported")
    if checkpoint.get("archiveId") != report.get("archiveId") or checkpoint.get(
        "artifactSetId"
    ) != report.get("artifactSetId"):
        raise ValueError("migration checkpoint identity does not match the inventory")
    if checkpoint.get("reportHash") != sha256(encoded_json(report)):
        raise ValueError("migration checkpoint report hash does not match")
    source_hashes = checkpoint.get("sourceHashes")
    artifact_hashes = checkpoint.get("artifactHashes")
    if not isinstance(source_hashes, dict) or not isinstance(artifact_hashes, dict):
        raise ValueError("migration checkpoint hash maps are malformed")
    expected_sources = {
        str(entry["path"]): str(entry["sha256"]) for entry in report["sources"]
    }
    if source_hashes != expected_sources:
        raise ValueError("migration checkpoint does not account for every source")
    if any(not re.fullmatch(r"[0-9a-f]{64}", str(value)) for value in artifact_hashes.values()):
        raise ValueError("migration checkpoint contains an invalid artifact hash")
    imported_stub = {
        str(entry["destination"]): ""
        for entry in report["sources"]
        if entry.get("disposition") == "imported revision"
    }
    validate_inventory(report, imported_stub, set(expected_sources))
    expected_artifacts = {
        "manifest.json",
        *(Path(path).relative_to(".agentum").as_posix() for path in imported_stub),
    }
    if set(artifact_hashes) != expected_artifacts:
        raise ValueError("migration checkpoint artifact set does not match the inventory")


def load_resume_state(
    root: Path, archive_root: Path, pattern_path: Path
) -> tuple[dict, dict, Path]:
    journal_path = root / JOURNAL_PATH
    if journal_path.is_symlink() or not journal_path.is_file():
        raise ValueError("migration journal is missing or unsafe")
    journal = json.loads(read_regular_no_follow(journal_path))
    if not isinstance(journal, dict):
        raise ValueError("migration journal is malformed")
    stage = journal.pop("stage", None)
    deleted_count = journal.pop("deletedCount", None)
    if stage not in {"archived", "published", "reported", "deleting"} or not isinstance(
        deleted_count, int
    ):
        raise ValueError("migration journal progress is malformed")
    archive_root = require_unsymlinked_absolute(archive_root, "archive directory")
    if archive_root == root or root in archive_root.parents:
        raise ValueError("archive directory must be outside the repository")
    archive = archive_root / str(journal.get("archiveId", ""))
    require_unsymlinked_absolute(archive, "archive")
    inventory_path = archive / "inventory.json"
    checkpoint_path = archive / "checkpoint.json"
    report_bytes = read_regular_no_follow(inventory_path)
    report = json.loads(report_bytes)
    if not isinstance(report, dict) or sha256(report_bytes) != journal.get("reportHash"):
        raise ValueError("recovery archive inventory does not match the journal")
    checkpoint = json.loads(read_regular_no_follow(checkpoint_path))
    if not isinstance(checkpoint, dict) or checkpoint != journal:
        raise ValueError("migration journal does not match the external checkpoint")
    validate_checkpoint_shape(checkpoint, report)
    if checkpoint["restrictedPatternHash"] != current_pattern_hash(root, pattern_path):
        raise ValueError("restricted-content policy changed during migration recovery")
    for source, expected_hash in checkpoint["sourceHashes"].items():
        archived = archive / Path(source)
        require_unsymlinked_absolute(archived.parent, "archive source parent")
        if sha256(read_regular_no_follow(archived)) != expected_hash:
            raise ValueError(f"archived source changed during migration recovery: {source}")
    return report, {**checkpoint, "stage": stage, "deletedCount": deleted_count}, archive


def validate_published_artifacts(root: Path, checkpoint: dict) -> None:
    artifact_root = root / ".agentum"
    validate_native_artifacts(root, artifact_root)
    expected = checkpoint["artifactHashes"]
    observed: dict[str, str] = {}
    for current, directories, files in os.walk(artifact_root, followlinks=False):
        current_path = Path(current)
        for name in directories:
            directory = current_path / name
            metadata = directory.lstat()
            if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
                raise ValueError(f"unsafe published artifact directory: {directory}")
        for name in files:
            path = current_path / name
            relative = path.relative_to(artifact_root).as_posix()
            observed[relative] = sha256(read_regular_no_follow(path))
    if observed != expected:
        raise ValueError("published .agentum content does not match the migration checkpoint")


def publish_native(
    root: Path,
    report: dict,
    imported: dict[str, str],
    pattern_path: Path,
) -> None:
    destination = root / ".agentum"
    if destination.exists() or destination.is_symlink():
        raise ValueError(".agentum already exists; refusing to overwrite")
    lock_path = root / ".agentum-migration.lock"
    lock_flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    lock_descriptor = os.open(lock_path, lock_flags, 0o600)
    lock_identity = os.fstat(lock_descriptor)
    stage = Path(tempfile.mkdtemp(prefix=".agentum-migration-stage-", dir=root))
    stage_identity = stage.stat()
    created_files: list[Path] = []
    created_directories: list[Path] = []
    published = False
    try:
        specs_directory = stage / "specs"
        specs_directory.mkdir()
        created_directories.append(specs_directory)
        manifest = manifest_bytes(report)
        manifest_path = stage / "manifest.json"
        write_exclusive_regular(manifest_path, manifest)
        created_files.append(manifest_path)
        for relative, content in sorted(imported.items()):
            inner = Path(relative).relative_to(".agentum")
            if (
                len(inner.parts) != 3
                or inner.parts[0] != "specs"
                or inner.name
                not in {"spec.md", "design.md", "plan.json", "decisions.md", "review.md"}
            ):
                raise ValueError(f"unexpected migration destination: {relative}")
            path = stage / inner
            if not path.parent.exists():
                path.parent.mkdir()
                created_directories.append(path.parent)
            write_exclusive_regular(path, content.encode())
            created_files.append(path)
            sync_directory(path.parent)
        sync_directory(specs_directory)
        sync_directory(stage)
        validate_native_artifacts(root, stage)
        stage_relative = stage.relative_to(root)
        if scan_restricted_matches(root, pattern_path, [stage_relative]):
            raise ValueError("generated .agentum artifacts contain restricted content")
        if destination.exists() or destination.is_symlink():
            raise ValueError(".agentum appeared during migration; refusing to overwrite")
        os.rename(stage, destination)
        published_identity = destination.stat()
        if (published_identity.st_dev, published_identity.st_ino) != (
            stage_identity.st_dev,
            stage_identity.st_ino,
        ):
            raise ValueError("published .agentum identity changed unexpectedly")
        published = True
        sync_directory(root)
    finally:
        os.close(lock_descriptor)
        try:
            lock_metadata = lock_path.lstat()
            if (lock_metadata.st_dev, lock_metadata.st_ino) == (
                lock_identity.st_dev,
                lock_identity.st_ino,
            ):
                lock_path.unlink()
        except FileNotFoundError:
            pass
        if not published and stage.exists() and not stage.is_symlink():
            current_identity = stage.stat()
            if (current_identity.st_dev, current_identity.st_ino) == (
                stage_identity.st_dev,
                stage_identity.st_ino,
            ):
                for path in reversed(created_files):
                    try:
                        path.unlink()
                    except FileNotFoundError:
                        pass
                for path in reversed(created_directories):
                    try:
                        path.rmdir()
                    except OSError:
                        pass
                try:
                    stage.rmdir()
                except OSError:
                    pass


def write_report_atomic(root: Path, report: dict) -> None:
    destination = root / REPORT_PATH
    destination.parent.mkdir(parents=True, exist_ok=True)
    require_unsymlinked_absolute(destination.parent, "migration report parent")
    report_data = (json.dumps(report, indent=2, sort_keys=False) + "\n").encode()
    if destination.exists() or destination.is_symlink():
        if read_regular_no_follow(destination) != report_data:
            raise ValueError("existing migration report does not match this preview")
        return
    temporary = destination.with_suffix(f".tmp-{os.getpid()}")
    write_exclusive_regular(temporary, report_data)
    os.replace(temporary, destination)


def validate_source_hashes(root: Path, report: dict) -> None:
    for entry in report["sources"]:
        read_source(root, Path(str(entry["path"])), str(entry["sha256"]))


def validate_resume_source_hashes(root: Path, report: dict) -> set[str]:
    owned_missing: set[str] = set()
    for entry in report["sources"]:
        relative = Path(str(entry["path"]))
        expected_hash = str(entry["sha256"])
        source = checked_relative_path(root, relative)
        if source.exists() or source.is_symlink():
            data = read_regular_no_follow(source)
        else:
            data = read_tracked_index_source(root, relative)
            owned_missing.add(relative.as_posix())
        if sha256(data) != expected_hash:
            raise ValueError(f"legacy source changed during migration recovery: {relative}")
    require_only_inventoried_sources(
        root,
        [Path(str(entry["path"])) for entry in report["sources"]],
        owned_missing=owned_missing,
    )
    return owned_missing


def remove_inventory_sources(root: Path, report: dict, on_deleted=None) -> int:
    # This is the final preimage check. Delete only the exact inventoried files;
    # ignored/untracked files that appear after the clean-tree check survive.
    owned_missing = validate_resume_source_hashes(root, report)
    deleted_count = len(owned_missing)
    for entry in report["sources"]:
        relative = Path(str(entry["path"]))
        source = checked_relative_path(root, relative)
        if relative.as_posix() in owned_missing:
            continue
        try:
            source.unlink()
        except FileNotFoundError:
            raise ValueError(f"legacy source disappeared during deletion: {relative}") from None
        deleted_count += 1
        if on_deleted is not None:
            on_deleted(deleted_count)

    for relative in LEGACY_DIRECTORIES:
        directory = root / relative
        if not directory.exists():
            continue
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f"unsafe legacy directory remained after migration: {relative}")
        for current, directories, _files in os.walk(directory, topdown=False, followlinks=False):
            current_path = Path(current)
            for name in directories:
                child = current_path / name
                if child.is_symlink():
                    raise ValueError(f"refusing to prune symlinked legacy directory: {child}")
                try:
                    child.rmdir()
                except OSError:
                    pass
            try:
                current_path.rmdir()
            except OSError:
                pass
    return deleted_count


def inject_crash(requested: str | None, point: str) -> None:
    if requested == point:
        raise OSError(f"injected migration crash at {point}")


def finish_apply(
    root: Path,
    report: dict,
    imported: dict[str, str] | None,
    journal: dict,
    pattern_path: Path,
    crash_point: str | None,
) -> None:
    checkpoint = {
        key: value for key, value in journal.items() if key not in {"stage", "deletedCount"}
    }
    artifact_root = root / ".agentum"
    report_path = root / REPORT_PATH

    if artifact_root.exists() or artifact_root.is_symlink():
        if artifact_root.is_symlink() or not artifact_root.is_dir():
            raise ValueError("published .agentum root is unsafe")
        validate_published_artifacts(root, checkpoint)
    else:
        if report_path.exists() or report_path.is_symlink():
            raise ValueError("migration report exists without its bound .agentum root")
        if imported is None:
            sources = [Path(str(entry["path"])) for entry in report["sources"]]
            restricted = scan_restricted_matches(root, pattern_path, sources)
            rebuilt_report, imported = build_inventory(root, sources, restricted)
            rebuilt = immutable_checkpoint(
                rebuilt_report, imported, current_pattern_hash(root, pattern_path)
            )
            if rebuilt_report != report or rebuilt != checkpoint:
                raise ValueError("legacy sources changed after the recovery checkpoint")
        validate_source_hashes(root, report)
        publish_native(root, report, imported, pattern_path)
        validate_published_artifacts(root, checkpoint)
    journal = write_journal(root, journal, "published", int(journal["deletedCount"]))
    inject_crash(crash_point, "post_publish")

    write_report_atomic(root, report)
    if read_regular_no_follow(report_path) != encoded_json(report):
        raise ValueError("published migration report does not match its checkpoint")
    journal = write_journal(root, journal, "reported", int(journal["deletedCount"]))
    inject_crash(crash_point, "post_report")

    validate_resume_source_hashes(root, report)
    injected_mid_delete = False

    def record_deletion(count: int) -> None:
        nonlocal journal, injected_mid_delete
        journal = write_journal(root, journal, "deleting", count)
        if not injected_mid_delete:
            injected_mid_delete = True
            inject_crash(crash_point, "mid_delete")

    deleted_count = remove_inventory_sources(root, report, record_deletion)
    journal = write_journal(root, journal, "deleting", deleted_count)
    require_only_inventoried_sources(root, [])
    validate_published_artifacts(root, checkpoint)
    if scan_restricted_matches(root, pattern_path, [Path(".agentum")]):
        raise ValueError("published .agentum artifacts contain restricted content")
    if read_regular_no_follow(report_path) != encoded_json(report):
        raise ValueError("migration report changed before completion")

    journal_path = root / JOURNAL_PATH
    if journal_path.is_symlink() or not journal_path.is_file():
        raise ValueError("migration journal disappeared before completion")
    journal_path.unlink()
    sync_directory(root)
    print(
        f"migrated {len(report['artifactAssociations'])} specifications; "
        f"accounted for {len(report['sources'])} sources"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--preview", action="store_true")
    mode.add_argument("--apply", action="store_true")
    parser.add_argument(
        "--repo-root",
        required=True,
        type=Path,
        help="absolute unsymlinked path to the exact repository being migrated",
    )
    parser.add_argument("--archive-dir", type=Path)
    parser.add_argument(
        "--restricted-patterns",
        type=Path,
        help="absolute external deny-pattern file; mandatory for apply",
    )
    parser.add_argument(
        "--test-crash-at",
        choices=("post_archive", "post_publish", "post_report", "mid_delete"),
        help=argparse.SUPPRESS,
    )
    args = parser.parse_args()

    root = explicit_repo_root(args.repo_root)
    if args.test_crash_at and os.environ.get("AGENTUM_MIGRATION_ENABLE_TEST_CRASH") != "1":
        parser.error("--test-crash-at is available only to the migration test suite")
    journal_path = root / JOURNAL_PATH
    if journal_path.exists() or journal_path.is_symlink():
        if not args.apply:
            raise ValueError("incomplete migration journal exists; resume with --apply")
        if args.archive_dir is None or not args.archive_dir.is_absolute():
            parser.error("migration recovery requires the original absolute --archive-dir")
        if args.restricted_patterns is None:
            parser.error("migration recovery requires the original --restricted-patterns")
        report, journal, _archive = load_resume_state(
            root, args.archive_dir, args.restricted_patterns
        )
        finish_apply(
            root,
            report,
            None,
            journal,
            args.restricted_patterns,
            args.test_crash_at,
        )
        return 0
    if (root / REPORT_PATH).is_file() and (root / ".agentum/manifest.json").is_file():
        legacy_present = any((root / path).exists() or (root / path).is_symlink() for path in LEGACY_ROOTS)
        if not legacy_present:
            require_only_inventoried_sources(root, [])
            print("legacy SDD migration is already complete; no-op")
            return 0
    tracked_sources = tracked_legacy_files(root)
    sources = list(tracked_sources)
    validate_tracked_demo_set(root, sources)
    if not sources:
        print("no tracked legacy SDD sources found")
        return 0

    if args.restricted_patterns is None:
        parser.error("--preview and --apply require an absolute external --restricted-patterns file")

    if (root / ".agentum").exists() or (root / ".agentum").is_symlink():
        raise ValueError(".agentum already exists; refusing to overwrite")
    if (root / REPORT_PATH).exists() or (root / REPORT_PATH).is_symlink():
        raise ValueError("migration report already exists without a completed migration")

    # Validate containment and file type before allowing the regex scanner to
    # inspect any source path; an explicit tracked symlink must never become an
    # out-of-repository read through the classification step.
    for source in sources:
        read_source(root, source)
    restricted_matches = scan_restricted_matches(root, args.restricted_patterns, sources)
    report, imported = build_inventory(root, sources, restricted_matches)
    if args.preview:
        json.dump(report, sys.stdout, indent=2)
        sys.stdout.write("\n")
        return 0
    if args.archive_dir is None or not args.archive_dir.is_absolute():
        parser.error("--apply requires an absolute --archive-dir outside the repository")
    if args.restricted_patterns is None:
        parser.error("--apply requires --restricted-patterns with an absolute external file")

    require_same_repo_root(root)
    require_clean_legacy_roots(root)
    require_only_inventoried_sources(root, sources)
    require_no_active_v1_run(root)
    validate_source_hashes(root, report)
    archive = ensure_external_archive(root, args.archive_dir, report)
    # Archive publication can take time on a large tree. Recheck every preimage
    # before publishing the new authoritative root.
    require_same_repo_root(root)
    require_clean_legacy_roots(root)
    require_only_inventoried_sources(root, sources)
    validate_source_hashes(root, report)
    checkpoint = immutable_checkpoint(
        report,
        imported,
        current_pattern_hash(root, args.restricted_patterns),
    )
    ensure_archive_checkpoint(archive, checkpoint)
    journal = create_journal(root, checkpoint)
    inject_crash(args.test_crash_at, "post_archive")
    finish_apply(
        root,
        report,
        imported,
        journal,
        args.restricted_patterns,
        args.test_crash_at,
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError, sqlite3.Error) as error:
        print(f"migration blocked: {error}", file=sys.stderr)
        raise SystemExit(1) from None
