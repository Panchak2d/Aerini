import base64
import importlib.util
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent.parent


def load(filename):
    spec = importlib.util.spec_from_file_location(filename.replace("-", "_").removesuffix(".py"), SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fake_sig(version, file_name="x"):
    trusted = f"timestamp:1700000000\tfile:{file_name}" + (f"\tversion:{version}" if version else "")
    text = (
        "untrusted comment: signature from tauri secret key\n"
        "RUQ" + "A" * 85 + "\n"
        f"trusted comment: {trusted}\n"
        + "B" * 86 + "\n"
    )
    return base64.b64encode(text.encode()).decode()
