"""Collect the expected bundles and verify updater signatures before drafting."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

source, output = map(Path, sys.argv[1:])
output.mkdir()
tag = os.environ["GITHUB_REF_NAME"]
if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
    raise SystemExit("Expected a stable version tag such as v0.4.0.")

patterns = {
    "linux-x86_64": "*.AppImage",
    "windows-x86_64": "*-setup.exe",
    "rpm": "*.rpm",
}
platforms = {}
public_key = os.environ.get("WORKSHOP_UPDATER_PUBLIC_KEY", "")
for target, pattern in patterns.items():
    files = list(source.rglob(pattern))
    if len(files) != 1:
        raise SystemExit(f"Expected exactly one {pattern} bundle, got {len(files)}.")
    bundle = files[0]
    suffix = {"linux-x86_64": "linux-x86_64.AppImage", "windows-x86_64": "windows-x86_64-setup.exe", "rpm": "linux-x86_64.rpm"}[target]
    name = f"workshop-downloader-{tag[1:]}-{suffix}"
    shutil.copyfile(bundle, output / name)
    if not public_key or target == "rpm":
        continue
    signature = bundle.with_name(bundle.name + ".sig")
    encoded = signature.read_text().strip()
    with tempfile.TemporaryDirectory() as scratch:
        key = Path(scratch) / "updater.pub"
        sig = Path(scratch) / "bundle.minisig"
        key.write_bytes(base64.b64decode(public_key.strip(), validate=True))
        signature_text = base64.b64decode(encoded, validate=True).decode("utf-8")
        sig.write_text(signature_text)
        trusted_comment = signature_text.splitlines()[2].removeprefix("trusted comment: ")
        if f"version:{tag[1:]}" not in trusted_comment.split("\t"):
            raise SystemExit(f"The signature for {bundle.name} is not bound to {tag[1:]}.")
        subprocess.run(
            ["minisign", "-Vm", str(bundle), "-p", str(key), "-x", str(sig)],
            check=True,
        )
    shutil.copyfile(signature, output / (name + ".sig"))
    platforms[target] = {
        "signature": encoded,
        "url": f"https://github.com/{os.environ['GITHUB_REPOSITORY']}/releases/download/{tag}/{name}",
    }

if platforms:
    metadata = {
        "version": tag[1:],
        "notes": Path("docs/release-notes.md").read_text(),
        "platforms": platforms,
    }
    (output / "latest.json").write_text(json.dumps(metadata, indent=2) + "\n")

checksums = []
for asset in sorted(output.iterdir()):
    digest = hashlib.sha256()
    with asset.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    checksums.append(f"{digest.hexdigest()}  {asset.name}")
(output / "SHA256SUMS.txt").write_text("\n".join(checksums) + "\n")
