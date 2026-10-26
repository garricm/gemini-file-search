"""Test release configuration and file recovery without GitHub access."""
import argparse
import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("homebrew_setup", ROOT / "scripts/configure_homebrew.py")
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


class HomebrewSetupTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.manifest = self.root / "Cargo.toml"
        self.original = '[package]\nname = "example"\nversion = "0.1.0"\n\n[dependencies]\nserde = "1"\n'
        self.manifest.write_text(self.original)
        (self.root / "packaging").mkdir()
        (self.root / "packaging/dist-workspace.toml.in").write_text((ROOT / "packaging/dist-workspace.toml.in").read_text())

    def test_rejects_urls_path_traversal_and_shell_fragments(self):
        for name in ["https://github.com/owner/repo", "owner/repo.git", "../repo", "owner/repo/extra", "owner/repo;touch file", "owner/$(cmd)"]:
            with self.subTest(name=name), self.assertRaises(argparse.ArgumentTypeError):
                setup.repository(name)
        with self.assertRaises(argparse.ArgumentTypeError):
            setup.tap("owner/ordinary-repo")

    def test_dry_run_changes_nothing_and_does_not_require_dist(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            setup.configure(self.root, "owner/app", "owner/homebrew-tap", "not-installed", dry_run=True)
        self.assertIn('tap = "owner/homebrew-tap"', output.getvalue())
        self.assertEqual(self.manifest.read_text(), self.original)
        self.assertFalse((self.root / "dist-workspace.toml").exists())
        self.assertFalse((self.root / ".github").exists())

    def test_preserves_custom_homepage_dependencies_and_idempotence(self):
        original = self.original.replace('[dependencies]', 'homepage = "https://example.org"\n\n[dependencies]')
        # Keep the custom homepage inside [package].
        modified = setup.cargo_metadata(original, "owner/app")
        parsed = setup.tomllib.loads(modified)
        self.assertEqual(parsed["package"]["homepage"], "https://example.org")
        self.assertEqual(parsed["package"]["repository"], "https://github.com/owner/app")
        self.assertEqual(parsed["dependencies"]["serde"], "1")
        self.assertEqual(setup.cargo_metadata(modified, "owner/app"), modified)

    def test_generator_failure_restores_files_and_removes_partial_workflow(self):
        workflow = self.root / ".github/workflows/release.yml"

        def generate(command, **kwargs):
            if command[1] == "--version":
                return subprocess.CompletedProcess(command, 0, stdout=f"cargo-dist {setup.DIST_VERSION}\n")
            workflow.parent.mkdir(parents=True, exist_ok=True)
            workflow.write_text("partial generated workflow")
            raise subprocess.CalledProcessError(1, command)

        with patch.object(setup.shutil, "which", return_value="/fake/dist"), patch.object(setup.subprocess, "run", side_effect=generate):
            with self.assertRaises(subprocess.CalledProcessError):
                setup.configure(self.root, "owner/app", "owner/homebrew-tap", "dist")
        self.assertEqual(self.manifest.read_text(), self.original)
        self.assertFalse((self.root / "dist-workspace.toml").exists())
        self.assertFalse(workflow.exists())

    def test_refuses_to_overwrite_a_handwritten_release_workflow(self):
        workflow = self.root / ".github/workflows/release.yml"
        workflow.parent.mkdir(parents=True)
        workflow.write_text("name: Existing release\n")
        with self.assertRaisesRegex(ValueError, "will not overwrite"):
            setup.configure(self.root, "owner/app", "owner/homebrew-tap", "dist")
        self.assertEqual(self.manifest.read_text(), self.original)
        self.assertEqual(workflow.read_text(), "name: Existing release\n")

    def test_workflow_text_preserves_license_and_commands(self):
        original = (
            "# Copyright 2022-2024, axodotdev\n"
            "# SPDX-License-Identifier: MIT or Apache-2.0\n"
            "# Original generator comment\n"
            "name: Release\n"
            "jobs:\n"
            "  build:\n"
            "    steps:\n"
            "      - run: dist build --artifacts=local\n"
        )
        result = setup.workflow_text(original)
        self.assertIn("# Copyright 2022-2024, axodotdev", result)
        self.assertIn("# SPDX-License-Identifier: MIT or Apache-2.0", result)
        self.assertNotIn("Original generator comment", result)
        self.assertIn("      - run: dist build --artifacts=local\n", result)
        self.assertEqual(setup.workflow_text(result), result)

    def test_check_finds_workflow_changes_and_restores_all_files(self):
        workflow = self.root / ".github/workflows/release.yml"

        def generate(command, **kwargs):
            if command[1] == "--version":
                return subprocess.CompletedProcess(command, 0, stdout=f"cargo-dist {setup.DIST_VERSION}\n")
            if command == ["/fake/dist", "generate"]:
                workflow.parent.mkdir(parents=True, exist_ok=True)
                workflow.write_text("name: Release\njobs: {}\n")
            return subprocess.CompletedProcess(command, 0)

        with patch.object(setup.shutil, "which", return_value="/fake/dist"), patch.object(setup.subprocess, "run", side_effect=generate):
            with contextlib.redirect_stdout(io.StringIO()):
                setup.configure(self.root, "owner/app", "owner/homebrew-tap", "dist")
                setup.configure(self.root, "owner/app", "owner/homebrew-tap", "dist", check=True)
            workflow.write_text(workflow.read_text().replace("name: Release", "name: Changed"))
            before = {path: path.read_bytes() for path in [self.manifest, self.root / "dist-workspace.toml", workflow]}
            with self.assertRaisesRegex(ValueError, "not current"):
                setup.configure(self.root, "owner/app", "owner/homebrew-tap", "dist", check=True)
            for path, value in before.items():
                self.assertEqual(path.read_bytes(), value)


if __name__ == "__main__":
    unittest.main()
