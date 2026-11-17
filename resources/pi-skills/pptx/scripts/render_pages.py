"""Use the shared Windows page renderer without duplicating the Office lifecycle logic."""
from pathlib import Path
import runpy

runpy.run_path(str(Path(__file__).resolve().parents[2] / "pdf" / "scripts" / "render_pages.py"), run_name="__main__")
