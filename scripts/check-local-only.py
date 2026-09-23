"""Check the temporary product-service pause without building Rust."""
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


assert "pub const LOCAL_ONLY_BUILD: bool = true;" in read("crates/proto/src/lib.rs")
for path in (
    "apps/skylark/src/auth_cli.rs", "apps/skylark/src/update_cli.rs",
    "crates/engine/src/lib.rs", "crates/engine/src/rpc.rs",
    "crates/ui/src/shell/sync_switch.rs", "crates/ui/src/shell/user_menu.rs",
    "crates/ui/src/state/queries.rs", "crates/ui/src/state/engine.rs",
):
    assert "LOCAL_ONLY_BUILD" in read(path), path
main = read("apps/skylark/src/main.rs")
assert "https://edge.zeron.sh" not in main
assert "client_01KWD0EAKZKD50YCQJNYSRE4BY" not in main
for name in ("deploy", "release", "testflight"):
    assert not (ROOT / f".github/workflows/{name}.yml").exists(), name
    assert (ROOT / f".github/workflows/{name}.yml.disabled").is_file(), name

deploy = json.loads(read("edge/package.json"))["scripts"]["deploy"]
assert "temporarily disabled" in deploy and "process.exit(1)" in deploy and "wrangler" not in deploy

installer = read("edge/src/install.sh")
guard, _ = installer.split("\nexit 1\n", 1)
assert "installation is temporarily disabled" in guard
assert "curl " not in guard and "systemctl " not in guard
result = subprocess.run(["sh", "edge/src/install.sh"], cwd=ROOT, capture_output=True, text=True, timeout=5, check=False)
assert result.returncode == 1 and "temporarily disabled" in result.stderr, result

active_docs = ("README.md", "README.zh-CN.md", "docs/RUNNING.md", "docs/LOCAL_ONLY.md", "dist/README.md")
for path in active_docs:
    assert "LOCAL_ONLY.md" in read(path) or path == "docs/LOCAL_ONLY.md", path
    assert not re.search(r"curl\s+[^\n]*zeron\.sh/install", read(path)), path
for path in (ROOT / "docs").rglob("*.md"):
    if path.name in ("RUNNING.md", "LOCAL_ONLY.md"):
        continue
    assert "Skylark build status: local-only" in path.read_text(encoding="utf-8"), path
print("PASS: local-only guards, paused publishing/installer, and documentation status")
