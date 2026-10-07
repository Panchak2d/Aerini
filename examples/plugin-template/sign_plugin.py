#!/usr/bin/env python3
"""
Generates and verifies the `<name>.wasm.sig` sidecar Aerini reads via
`check_signature` (src-tauri/src/commands/plugins.rs). See "Signing your
plugin" in docs/development/plugin-authoring.md for the full walkthrough.

Usage:
    python sign_plugin.py keygen --out publisher.key
    python sign_plugin.py sign --key publisher.key --wasm plugin.wasm
    python sign_plugin.py verify --sig plugin.wasm.sig

Requires: pip install blake3 cryptography
"""
import argparse
import base64
import json
import sys
from pathlib import Path

import blake3
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)

SCHEMA_VERSION = 1
ALGORITHM = "ed25519"
DOMAIN = b"aerini-plugin-sig-v1\n"


def signed_message(files: list[dict]) -> bytes:
    """Mirrors plugins.rs::signed_message exactly: domain prefix, then each
    entry's name + 0x00 + blake3 hex + newline, sorted by name."""
    msg = bytearray(DOMAIN)
    for entry in sorted(files, key=lambda e: e["name"]):
        msg += entry["name"].encode("utf-8")
        msg += b"\x00"
        msg += entry["blake3"].encode("utf-8")
        msg += b"\n"
    return bytes(msg)


def cmd_keygen(args: argparse.Namespace) -> None:
    key = Ed25519PrivateKey.generate()
    raw = key.private_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PrivateFormat.Raw,
        encryption_algorithm=serialization.NoEncryption(),
    )
    out = Path(args.out)
    out.write_bytes(raw)
    try:
        out.chmod(0o600)
    except OSError:
        pass  # best-effort on platforms without POSIX permission bits
    pub_b64 = base64.b64encode(
        key.public_key().public_bytes(
            encoding=serialization.Encoding.Raw,
            format=serialization.PublicFormat.Raw,
        )
    ).decode("ascii")
    print(f"Wrote private key: {out}")
    print(f"Public key (base64): {pub_b64}")
    print("Keep the private key file secret.")


def cmd_sign(args: argparse.Namespace) -> None:
    key_bytes = Path(args.key).read_bytes()
    if len(key_bytes) != 32:
        sys.exit(f"error: {args.key} is {len(key_bytes)} bytes, expected 32 (raw Ed25519 private key)")
    signing_key = Ed25519PrivateKey.from_private_bytes(key_bytes)

    wasm_path = Path(args.wasm)
    wasm_bytes = wasm_path.read_bytes()
    digest = blake3.blake3(wasm_bytes).hexdigest()

    files = [{"name": wasm_path.name, "blake3": digest}]
    signature = signing_key.sign(signed_message(files))
    public_key = signing_key.public_key().public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw,
    )

    sig_doc = {
        "schema_version": SCHEMA_VERSION,
        "algorithm": ALGORITHM,
        "public_key": base64.b64encode(public_key).decode("ascii"),
        "files": files,
        "signature": base64.b64encode(signature).decode("ascii"),
    }

    sig_path = wasm_path.with_name(wasm_path.name + ".sig")
    sig_path.write_text(json.dumps(sig_doc, indent=2) + "\n")
    print(f"Wrote {sig_path}")


def cmd_verify(args: argparse.Namespace) -> None:
    sig_path = Path(args.sig)
    try:
        doc = json.loads(sig_path.read_text())
    except (OSError, json.JSONDecodeError) as e:
        sys.exit(f"error: cannot read/parse {sig_path}: {e}")

    if doc.get("schema_version") != SCHEMA_VERSION or doc.get("algorithm") != ALGORITHM:
        sys.exit("error: unrecognized schema_version/algorithm")
    files = doc.get("files")
    if not isinstance(files, list) or len(files) != 1:
        sys.exit("error: 'files' must contain exactly one entry for a standalone plugin")

    wasm_path = sig_path.with_name(files[0]["name"])
    if not wasm_path.exists():
        sys.exit(f"error: referenced file not found next to sidecar: {wasm_path}")
    actual_hash = blake3.blake3(wasm_path.read_bytes()).hexdigest()
    if actual_hash != files[0]["blake3"]:
        sys.exit(f"FAIL: integrity mismatch for {wasm_path.name}")

    try:
        public_key_bytes = base64.b64decode(doc["public_key"])
        signature_bytes = base64.b64decode(doc["signature"])
    except Exception as e:
        sys.exit(f"error: public_key/signature is not valid base64: {e}")
    if len(public_key_bytes) != 32:
        sys.exit(f"error: public_key is {len(public_key_bytes)} bytes, expected 32")
    if len(signature_bytes) != 64:
        sys.exit(f"error: signature is {len(signature_bytes)} bytes, expected 64")

    try:
        verifying_key = Ed25519PublicKey.from_public_bytes(public_key_bytes)
    except Exception as e:
        sys.exit(f"error: public_key is not a valid Ed25519 point: {e}")

    try:
        verifying_key.verify(signature_bytes, signed_message(files))
    except InvalidSignature:
        sys.exit("FAIL: signature does not verify")

    print(f"OK: {wasm_path.name} matches its signature and content hash.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    p_keygen = sub.add_parser("keygen", help="Generate a new publisher keypair.")
    p_keygen.add_argument("--out", required=True, help="Path to write the 32-byte raw private key.")
    p_keygen.set_defaults(func=cmd_keygen)

    p_sign = sub.add_parser("sign", help="Sign a .wasm file, writing <wasm>.sig next to it.")
    p_sign.add_argument("--key", required=True, help="Path to the raw private key (from keygen).")
    p_sign.add_argument("--wasm", required=True, help="Path to the .wasm file to sign.")
    p_sign.set_defaults(func=cmd_sign)

    p_verify = sub.add_parser("verify", help="Verify a .wasm.sig sidecar against its .wasm file.")
    p_verify.add_argument("--sig", required=True, help="Path to the .wasm.sig sidecar.")
    p_verify.set_defaults(func=cmd_verify)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
