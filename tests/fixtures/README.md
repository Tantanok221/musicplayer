# Audio fixtures

The four audio files contain two consecutive 100 ms segments of a generated 440 Hz sine wave at 48 kHz, stereo, 16-bit PCM amplitude 10000. Each segment contains exactly 4800 frames. They were encoded independently with FFmpeg's `flac` and `libmp3lame` encoders.

They contain no recorded or copyrighted music. FLAC tests check exact sample count and phase continuity; MP3 tests check gapless encoder-delay/padding trimming and seeking.

Native-format streaming tests also generate WAV data in memory at 44.1 kHz mono, 96 kHz stereo, and 48 kHz stereo. A local HTTP fixture serves the original files; tests verify exact decoded samples and sample counts, native formats despite incorrect server metadata, and format transitions without dropping the first packet. The 48 kHz fixture rate is not a playback requirement.

Bluetooth compatibility fixtures contain another 100 ms generated 440 Hz sine wave, encoded with FFmpeg: 96 kHz stereo 24-bit FLAC (`tone-96k.flac`), 48 kHz stereo AAC (`tone-aac.m4a`), 96 kHz stereo ALAC (`tone-alac.m4a`), 96 kHz mono 24-bit AIFF (`tone-96k.aiff`), 192 kHz stereo float WAV (`tone-192k.wav`), and 44.1 kHz mono Ogg/Vorbis (`tone-vorbis.ogg`). Tests check decoding, finite non-silent output, and duration after conversion to 44.1 kHz. AAC encoder padding is retained where the decoder exposes it; AAC gapless playback is not asserted.
