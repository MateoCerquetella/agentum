#!/usr/bin/env python3
"""Validate the complete project-owned .agentum artifact boundary."""

from __future__ import annotations

from collections import Counter
import json
import os
from pathlib import Path
import re
import stat
import sys


ULID_UPPER = re.compile(r"^[0-7][0-9A-HJKMNP-TV-Z]{25}$")
DIRECTORY = re.compile(r"^spc-([0-7][0-9a-hjkmnp-tv-z]{25})-[a-z0-9]+(?:-[a-z0-9]+)*$")
ALLOWED_ARTIFACTS = {"spec.md", "design.md", "plan.json", "decisions.md", "review.md"}


def fail(message: str) -> None:
    raise ValueError(message)


def require_regular(path: Path, label: str) -> None:
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        fail(f"missing {label}: {path}")
    if not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a real regular file: {path}")


def require_directory(path: Path, label: str) -> None:
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        fail(f"missing {label}: {path}")
    if not stat.S_ISDIR(metadata.st_mode):
        fail(f"{label} must be a real directory: {path}")


def parse_frontmatter(path: Path) -> tuple[dict[str, str], str]:
    content = path.read_text(encoding="utf-8")
    if not content.startswith("---\n") or "\n---\n" not in content[4:]:
        fail(f"spec.md has malformed frontmatter: {path}")
    header, body = content[4:].split("\n---\n", 1)
    fields: dict[str, str] = {}
    for line in header.splitlines():
        if ":" not in line:
            fail(f"malformed frontmatter line in {path}: {line}")
        key, value = (part.strip() for part in line.split(":", 1))
        if key in fields or key not in {"schema", "id", "revision", "title", "source"}:
            fail(f"unknown or duplicate frontmatter field in {path}: {key}")
        if not value:
            fail(f"empty frontmatter field in {path}: {key}")
        fields[key] = value
    if set(fields) - {"source"} != {"schema", "id", "revision", "title"}:
        fail(f"spec.md is missing canonical frontmatter fields: {path}")
    return fields, body


def collect_ids(body: str, prefix: str) -> list[str]:
    found = []
    for line in body.splitlines():
        trimmed = line.lstrip("-* \t")
        match = re.match(re.escape(prefix) + r"(\d+)", trimmed)
        if match:
            found.append(prefix + match.group(1))
    return found


def validate_plan(path: Path, spec_id: str, revision: int) -> None:
    try:
        plan = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"invalid plan.json at {path}: {error}")
    if not isinstance(plan, dict):
        fail(f"plan.json must be an object: {path}")
    if plan.get("schemaVersion") != 1 or plan.get("specId") != spec_id or plan.get("specRevision") != revision:
        fail(f"plan.json identity or revision mismatch: {path}")
    tasks = plan.get("tasks")
    if not isinstance(tasks, list):
        fail(f"plan.json tasks must be an array: {path}")
    ids = [task.get("id") for task in tasks if isinstance(task, dict)]
    if len(ids) != len(tasks) or any(not isinstance(value, str) or not value.strip() for value in ids):
        fail(f"plan.json task IDs must be non-empty: {path}")
    if len(set(ids)) != len(ids):
        fail(f"plan.json task IDs must be unique: {path}")
    known = set(ids)
    for task in tasks:
        dependencies = task.get("dependencies", [])
        if not isinstance(dependencies, list) or any(value not in known for value in dependencies):
            fail(f"plan.json has an unknown task dependency: {path}")


def validate(root: Path) -> None:
    root = Path(os.path.abspath(root))
    require_directory(root, ".agentum root")
    root_entries = {entry.name: entry for entry in root.iterdir()}
    if set(root_entries) != {"manifest.json", "specs"}:
        fail(".agentum root must contain only manifest.json and specs/")

    manifest_path = root_entries["manifest.json"]
    specs_path = root_entries["specs"]
    require_regular(manifest_path, "manifest")
    require_directory(specs_path, "specs root")
    spec_directories = list(specs_path.iterdir())
    if not spec_directories:
        fail("specs root must contain at least one saved specification")
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"invalid manifest.json: {error}")
    if (
        not isinstance(manifest, dict)
        or set(manifest) != {"format", "schemaVersion", "artifactSetId"}
        or manifest.get("format") != "agentum-sdd"
        or manifest.get("schemaVersion") != 1
        or not isinstance(manifest.get("artifactSetId"), str)
        or not ULID_UPPER.fullmatch(manifest["artifactSetId"])
    ):
        fail("invalid static .agentum manifest")

    for directory in spec_directories:
        require_directory(directory, "spec directory")
        match = DIRECTORY.fullmatch(directory.name)
        if not match:
            fail(f"invalid spec directory name: {directory.name}")
        entries = {entry.name: entry for entry in directory.iterdir()}
        if "spec.md" not in entries:
            fail(f"spec directory has no spec.md: {directory.name}")
        unexpected = set(entries) - ALLOWED_ARTIFACTS
        if unexpected:
            fail(f"unexpected artifact in {directory.name}: {sorted(unexpected)[0]}")
        for name, artifact in entries.items():
            require_regular(artifact, f"{name} artifact")
            if name.endswith(".md") and not artifact.read_text(encoding="utf-8").strip():
                fail(f"empty Markdown artifact: {artifact}")

        fields, body = parse_frontmatter(entries["spec.md"])
        if fields["schema"] != "1":
            fail(f"unsupported spec schema: {directory.name}")
        spec_id = fields["id"]
        if not spec_id.startswith("SPC-") or not ULID_UPPER.fullmatch(spec_id[4:]):
            fail(f"invalid canonical spec ID: {directory.name}")
        if match.group(1).upper() != spec_id[4:]:
            fail(f"spec path ULID does not match frontmatter ID: {directory.name}")
        try:
            revision = int(fields["revision"])
        except ValueError:
            fail(f"invalid spec revision: {directory.name}")
        if revision < 1:
            fail(f"invalid spec revision: {directory.name}")

        for prefix in ("RQ-", "AC-"):
            identifiers = collect_ids(body, prefix)
            if not identifiers:
                fail(f"spec has no stable {prefix} identifiers: {directory.name}")
            duplicates = [value for value, count in Counter(identifiers).items() if count > 1]
            if duplicates:
                fail(f"duplicate spec identifier {duplicates[0]}: {directory.name}")
        if "plan.json" in entries:
            validate_plan(entries["plan.json"], spec_id, revision)


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {Path(sys.argv[0]).name} /path/to/.agentum", file=sys.stderr)
        return 2
    try:
        validate(Path(sys.argv[1]))
    except (OSError, ValueError) as error:
        print(f"invalid .agentum artifact root: {error}", file=sys.stderr)
        return 1
    print(".agentum artifact root: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
