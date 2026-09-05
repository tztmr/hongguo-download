from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]


class AiRuntimeDependencyTests(unittest.TestCase):
    def test_torch_and_torchaudio_are_locked_to_a_demucs_compatible_pair(self):
        requirements = (ROOT / "requirements-ai.txt").read_text(encoding="utf-8")
        torch = re.search(r"^torch==(\d+\.\d+\.\d+)$", requirements, re.MULTILINE)
        torchaudio = re.search(
            r"^torchaudio==(\d+\.\d+\.\d+)$", requirements, re.MULTILINE
        )
        soundfile = re.search(
            r"^soundfile==(\d+\.\d+\.\d+)$", requirements, re.MULTILINE
        )
        numpy = re.search(r"^numpy==(\d+\.\d+\.\d+)$", requirements, re.MULTILINE)

        self.assertIsNotNone(torch)
        self.assertIsNotNone(torchaudio)
        self.assertIsNotNone(soundfile)
        self.assertIsNotNone(numpy)
        self.assertEqual(torch.group(1), torchaudio.group(1))
        self.assertLess(tuple(map(int, torch.group(1).split("."))), (2, 6, 0))
        self.assertLess(tuple(map(int, numpy.group(1).split("."))), (2, 0, 0))


if __name__ == "__main__":
    unittest.main()
