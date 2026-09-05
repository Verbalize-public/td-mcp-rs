#!/usr/bin/env python3
"""Check local Markdown file links and heading anchors without network access."""

from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[1]


def prose(path):
    return re.sub(r"^```[^\n]*\n.*?^```[^\n]*$", "", path.read_text(encoding="utf-8"), flags=re.M | re.S)


def anchors(path):
    counts = {}
    result = set()
    for heading in re.findall(r"^#{1,6}\s+(.+?)\s*#*\s*$", prose(path), re.M):
        heading = re.sub(r"\[([^]]+)\]\([^)]*\)", r"\1", heading)
        slug = re.sub(r"[^\w\- ]", "", heading.lower()).replace(" ", "-")
        count = counts.get(slug, 0)
        counts[slug] = count + 1
        result.add(f"{slug}-{count}" if count else slug)
    result.update(re.findall(r'(?:id|name)="([^"]+)"', prose(path)))
    return result


def check():
    failures = []
    paths = sorted(ROOT.glob("*.md")) + sorted((ROOT / "docs").glob("*.md")) + [ROOT / "skills/README.md"]
    for source in paths:
        for target in re.findall(r"\[[^]\n]*\]\(([^)\n]+)\)", prose(source)):
            target = target.strip().split(' "', 1)[0].strip("<>")
            parsed = urlsplit(target)
            if parsed.scheme or parsed.netloc:
                continue
            dest = (source.parent / unquote(parsed.path)).resolve() if parsed.path else source
            if not dest.exists():
                failures.append(f"{source.relative_to(ROOT)}: missing {target}")
            elif parsed.fragment and dest.suffix == ".md" and unquote(parsed.fragment) not in anchors(dest):
                failures.append(f"{source.relative_to(ROOT)}: missing heading {target}")
    print("\n".join(failures) if failures else f"Documentation links passed ({len(paths)} files)")
    return bool(failures)


if __name__ == "__main__":
    sys.exit(check())
