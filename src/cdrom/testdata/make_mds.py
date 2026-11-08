"""Writes toolkit-dpm.mds and toolkit-plain.mds, the MDS descriptors the
CD image tests read (`cdrom::image::tests::toolkit_mds_images`), with the
disc archiver of refurbished-dinosaurs-toolkit (commit f32bdba, PR #554):
a data track of 31 sectors and an audio track of 300 after a pregap of
150, with DPM data the archiver measured on its simulated drive, and the
same disc without it. The tests make the .mdf themselves.

Run it with the toolkit's packages/disc-archiver/src and
packages/disc-archiver/tests folders on PYTHONPATH; the first argument is
the folder to write to. It prints the DPM data, which the tests check.
"""

import json
import sys
from pathlib import Path

from fakedrive import FakeDisc, FakeDrive
from synthetic import SyntheticDisc, iso_image

from dinorefurb_disc_archiver import pipeline
from dinorefurb_disc_archiver.cue import read_cue
from dinorefurb_disc_archiver.dpmmeasure import Settings, measure

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
synthetic = SyntheticDisc(iso_image({"README.TXT": b"rust-dos MDS fixture\r\n"}), audio_lengths=[300])
disc = read_cue(synthetic.write_split(out / "source"))

# Sectors 200 to 399 are 3% longer than a uniform disc's, as a pressing
# whose sectors a protection checks might have them.
drive = FakeDrive(FakeDisc([(200, 400, 0.03)]))
result = measure(drive, drive.clock, 0, disc.total_sectors, Settings(resolution=10), lambda _: None)
assert result.dpm is not None, result.record
disc.dpm = result.dpm
pipeline.write_format("mds", disc, out / "dpm", "Synth", lambda _: None)
(out / "dpm" / "Synth.mds").replace(out / "toolkit-dpm.mds")
disc.dpm = None
pipeline.write_format("mds", disc, out / "plain", "Synth", lambda _: None)
(out / "plain" / "Synth.mds").replace(out / "toolkit-plain.mds")
d = result.dpm
print(json.dumps({"start": d.start, "resolution": d.resolution, "angles": list(d.angles)}))
