#!/usr/bin/env python3
"""打包 GitHub Release 资产。"""

from __future__ import annotations

import argparse
import shutil
import tarfile
import zipfile
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Package llm-wiki release assets")
    parser.add_argument("--repo-root", required=True)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--package-id")
    parser.add_argument("--tag", required=True)
    parser.add_argument("--archive-format", choices=("tar.gz", "zip"), required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--package-name", default="llm-wiki")
    return parser.parse_args()


def copy_tree_file(repo_root: Path, relative_path: str, package_root: Path) -> None:
    source = repo_root / relative_path
    destination = package_root / relative_path
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)


def build_package_root(
    repo_root: Path,
    binary_path: Path,
    package_id: str,
    tag: str,
    output_dir: Path,
    package_name: str,
) -> Path:
    package_root = output_dir / f"{package_name}-{tag}-{package_id}"
    if package_root.exists():
        shutil.rmtree(package_root)
    package_root.mkdir(parents=True)

    shutil.copy2(binary_path, package_root / binary_path.name)

    for relative_path in (
        "README.md",
        "config/llm_wiki.template.toml",
        "docs/mcp_interface.md",
        "model/fetch_fastembed_model.sh",
        "model/fetch_fastembed_model.ps1",
        "systemd/llm-wiki-index.service",
        "systemd/llm-wiki-mcp.service",
    ):
        copy_tree_file(repo_root, relative_path, package_root)

    return package_root


def create_archive(package_root: Path, archive_format: str) -> Path:
    if archive_format == "tar.gz":
        archive_path = package_root.parent / f"{package_root.name}.tar.gz"
        with tarfile.open(archive_path, "w:gz") as archive:
            archive.add(package_root, arcname=package_root.name)
        return archive_path

    archive_path = package_root.parent / f"{package_root.name}.zip"
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in package_root.rglob("*"):
            archive.write(path, arcname=path.relative_to(package_root.parent))
    return archive_path


def main() -> int:
    args = parse_args()
    repo_root = Path(args.repo_root).resolve()
    binary_path = Path(args.binary).resolve()
    output_dir = Path(args.output_dir).resolve()

    if not binary_path.is_file():
        raise FileNotFoundError(f"binary not found: {binary_path}")

    output_dir.mkdir(parents=True, exist_ok=True)
    package_id = args.package_id or args.target
    package_root = build_package_root(
        repo_root=repo_root,
        binary_path=binary_path,
        package_id=package_id,
        tag=args.tag,
        output_dir=output_dir,
        package_name=args.package_name,
    )
    archive_path = create_archive(package_root, args.archive_format)
    print(archive_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
