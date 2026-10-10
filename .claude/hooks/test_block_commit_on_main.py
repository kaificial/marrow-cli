"""Tests for block_commit_on_main.py: python -m unittest discover -s .claude/hooks"""

import os
import shutil
import subprocess
import tempfile
import unittest

from block_commit_on_main import blocked


def repository(parent, name, branch):
    path = os.path.join(parent, name)
    os.makedirs(path)
    subprocess.run(["git", "init", "-q", "-b", branch, path], check=True)
    return path


def bash_path(path):
    """How Git Bash spells a Windows path: C:\\Users\\me becomes /c/Users/me."""
    if os.name == "nt" and len(path) > 1 and path[1] == ":":
        return "/" + path[0].lower() + path[2:].replace("\\", "/")
    return path


class BlockCommitOnMainTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.parent = tempfile.mkdtemp()
        cls.main = repository(cls.parent, "on-main", "main")
        cls.feature = repository(cls.parent, "on-feature", "feature")
        cls.plain = os.path.join(cls.parent, "not-a-repository")
        os.makedirs(cls.plain)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.parent, ignore_errors=True)

    def test_a_commit_where_the_agent_is_follows_that_repository(self):
        self.assertIsNotNone(blocked('git commit -m "x"', self.main))
        self.assertIsNone(blocked('git commit -m "x"', self.feature))

    def test_cd_moves_the_commit_to_another_repository(self):
        # The bug this hook had: it asked the repository it lives in, wherever the commit ran.
        self.assertIsNone(blocked(f'cd "{self.feature}" && git commit -m x', self.main))
        self.assertIsNotNone(blocked(f'cd "{self.main}" && git commit -m x', self.feature))
        self.assertIsNotNone(blocked(f"cd {bash_path(self.main)} && git commit -m x", self.feature))
        self.assertIsNotNone(blocked("cd ../on-main && git commit -m x", self.feature))

    def test_git_dash_c_names_the_repository(self):
        # Also missed before: "git -C dir commit" never contains the text "git commit".
        self.assertIsNotNone(blocked(f'git -C "{self.main}" commit -m x', self.feature))
        self.assertIsNone(blocked(f'git -C "{self.feature}" commit -m x', self.main))
        self.assertIsNotNone(blocked("git -c user.name=me commit -m x", self.main))

    def test_only_the_commit_subcommand_counts(self):
        for command in [
            "git log --grep commit",
            'echo "git commit"',
            "git status && echo commit",
            "GIT_PAGER=cat git show HEAD --stat",
        ]:
            self.assertIsNone(blocked(command, self.main), command)

    def test_a_commit_with_a_heredoc_message_is_still_seen(self):
        command = "git add -A && git commit -m \"$(cat <<'EOF'\nfeat: x\n\nbody\nEOF\n)\""
        self.assertIsNotNone(blocked(command, self.main))

    def test_outside_any_repository_nothing_is_blocked(self):
        self.assertIsNone(blocked("git commit -m x", self.plain))


if __name__ == "__main__":
    unittest.main()
