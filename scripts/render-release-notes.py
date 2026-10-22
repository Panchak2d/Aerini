#!/usr/bin/env python3
"""Render a release's download list, grouped by system, into its notes.

Usage:
  render-release-notes.py --tag vX.Y.Z --repo OWNER/NAME --assets assets.txt \
      --out notes.md [--body current-notes.md]

assets.txt holds one release asset name per line, exactly as GitHub stores it.
--body is the notes the release has now. Text outside the markers is kept as it is, and a
download list from an earlier run is replaced, so running this again gives the same result.

Every asset appears exactly once. A name that is not recognised is listed under "Other
files" instead of being dropped.
"""

import argparse
import re
import sys
from pathlib import Path
from urllib.parse import quote

START = "<!-- downloads:start -->"
END = "<!-- downloads:end -->"

TAG_RE = re.compile(r"^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")
REPO_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
ARCH_RE = re.compile(r"(?<![A-Za-z0-9])(x86_64|amd64|x64|aarch64|arm64)(?![A-Za-z0-9])")
MCP_RE = re.compile(r"^(aerini-mcp|aerini-mcp-offline)_([^_]+)_(.+?)(\.tar\.gz|\.zip)(\.sha256)?$")

OS_ORDER = ("Windows", "macOS", "Linux")
ARCH_ORDER = ("x64", "ARM64")
ARCH_NAMES = {
    "x86_64": "x64",
    "amd64": "x64",
    "x64": "x64",
    "aarch64": "ARM64",
    "arm64": "ARM64",
}
MAC_ARCH_NAMES = {"x64": "Intel", "ARM64": "Apple Silicon"}

INSTALLER_SUFFIXES = (
    (".app.tar.gz", "macOS", "app"),
    (".AppImage", "Linux", "appimage"),
    (".dmg", "macOS", "dmg"),
    (".deb", "Linux", "deb"),
    (".rpm", "Linux", "rpm"),
    (".exe", "Windows", "exe"),
    (".msi", "Windows", "msi"),
)
FORMAT_ORDER = ("exe", "msi", "dmg", "app", "appimage", "deb", "rpm")
FORMAT_NOTES = {
    "exe": "Installer (.exe)",
    "msi": "Installer (.msi)",
    "dmg": "Disk image (.dmg)",
    "app": "Compressed app bundle (.app.tar.gz)",
    "appimage": "AppImage: runs on most distributions, no install step",
    "deb": "Debian, Ubuntu and derivatives",
    "rpm": "Fedora, RHEL, openSUSE and derivatives",
}


class RenderError(Exception):
    pass


def arch_label(os_name, arch):
    return MAC_ARCH_NAMES[arch] if os_name == "macOS" else arch


def mcp_platform(target):
    parts = target.split("-")
    arch = ARCH_NAMES.get(parts[0])
    if arch is None:
        return None
    for marker, os_name in (("windows", "Windows"), ("darwin", "macOS"), ("linux", "Linux")):
        if marker in parts:
            return os_name, arch
    return None


def classify(name):
    """Return a tuple describing the asset, or None if it is not recognised."""
    if name == "latest.json":
        return ("manifest",)
    if name.endswith(".sig"):
        return ("signature",)
    match = MCP_RE.match(name)
    if match:
        program, _, target, _, checksum = match.groups()
        platform = mcp_platform(target)
        if platform:
            return ("mcp", program, platform, "checksum" if checksum else "archive")
    for suffix, os_name, fmt in INSTALLER_SUFFIXES:
        if name.endswith(suffix):
            archs = ARCH_RE.findall(name[: -len(suffix)])
            if archs:
                return ("installer", os_name, ARCH_NAMES[archs[-1]], fmt)
            return None
    return None


def render(tag, repo, names):
    if not TAG_RE.match(tag):
        raise RenderError(f"tag '{tag}' must look like vMAJOR.MINOR.PATCH[-prerelease]")
    if not REPO_RE.match(repo):
        raise RenderError(f"repository '{repo}' must look like OWNER/NAME")

    base = f"https://github.com/{repo}/releases/download/{quote(tag, safe='')}"

    def url(name):
        return f"{base}/{quote(name, safe='')}"

    def link(name):
        return f"[`{name}`]({url(name)})"

    installers = {}
    mcp = {}
    manifests, signatures, other = [], [], []
    for name in sorted(set(names)):
        kind = classify(name)
        if kind is None:
            other.append(name)
        elif kind[0] == "installer":
            _, os_name, arch, fmt = kind
            installers.setdefault(os_name, []).append((FORMAT_ORDER.index(fmt), ARCH_ORDER.index(arch), name, arch, fmt))
        elif kind[0] == "mcp":
            _, program, platform, part = kind
            mcp.setdefault(platform, {}).setdefault(program, {})[part] = name
        elif kind[0] == "manifest":
            manifests.append(name)
        else:
            signatures.append(name)

    for cells in mcp.values():
        for program in cells.values():
            if "archive" not in program:
                other.append(program.pop("checksum"))
    other.sort()

    out = ["## Downloads", "", "Pick your system below. Files for the in-app updater are at the bottom."]

    for os_name in OS_ORDER:
        if os_name not in installers:
            continue
        out += ["", f"### {os_name}", "", "| Download | Architecture | Notes |", "|---|---|---|"]
        for _, _, name, arch, fmt in sorted(installers[os_name]):
            out.append(f"| {link(name)} | {arch_label(os_name, arch)} | {FORMAT_NOTES[fmt]} |")

    mcp = {platform: cells for platform, cells in mcp.items() if any(cells.values())}
    if mcp:
        out += [
            "",
            "### MCP server (`aerini-mcp`)",
            "",
            "For connecting an AI agent to Aerini. **Offline** needs no server: the agent saves workflows "
            "as files you open in the desktop app. **Online** connects to an Aerini server. "
            f"See the [setup guide](https://github.com/{repo}/blob/main/docs/operations/mcp.md).",
            "",
            "| Platform | Online | Offline |",
            "|---|---|---|",
        ]
        for platform in sorted(mcp, key=lambda p: (OS_ORDER.index(p[0]), ARCH_ORDER.index(p[1]))):
            row = [f"{platform[0]} {arch_label(*platform)}"]
            for program in ("aerini-mcp", "aerini-mcp-offline"):
                cell = mcp[platform].get(program, {})
                if "archive" not in cell:
                    row.append("-")
                    continue
                archive = cell["archive"]
                text = f"[{'.zip' if archive.endswith('.zip') else '.tar.gz'}]({url(archive)})"
                if "checksum" in cell:
                    text += f" · [.sha256]({url(cell['checksum'])})"
                row.append(text)
            out.append("| " + " | ".join(row) + " |")

    if other:
        out += ["", "### Other files", ""] + [f"- {link(name)}" for name in other]

    if manifests or signatures:
        out += ["", "<details>", "<summary>Auto-update files (not needed to install)</summary>", ""]
        out.append("The desktop app reads these to find and verify updates.")
        out.append("")
        out += [f"- {link(name)}" for name in manifests + sorted(signatures)]
        out += ["", "</details>"]

    return "\n".join([START, *out, END])


def merge(body, block):
    body = body.replace("\r\n", "\n")
    starts, ends = body.count(START), body.count(END)
    if (starts, ends) == (0, 0):
        head = body.rstrip()
        return (head + "\n\n" if head else "") + block + "\n"
    if (starts, ends) != (1, 1) or body.index(START) > body.index(END):
        raise RenderError(
            "the release notes hold unmatched or repeated download markers; fix them by hand and run again"
        )
    before, rest = body.split(START, 1)
    _, after = rest.split(END, 1)
    return before + block + after


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--assets", required=True, help="file with one asset name per line")
    parser.add_argument("--body", help="file holding the current release notes")
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)

    try:
        names = [
            line.strip()
            for line in Path(args.assets).read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        current = Path(args.body).read_text(encoding="utf-8") if args.body else ""
        notes = merge(current, render(args.tag, args.repo, names))
    except (RenderError, OSError) as err:
        print(f"ERROR: {err}", file=sys.stderr)
        return 1

    Path(args.out).write_text(notes, encoding="utf-8")
    print(f"listed {len(names)} assets for {args.tag}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
