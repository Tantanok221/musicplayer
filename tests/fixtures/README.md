# Audio fixtures

The four audio files contain two consecutive 100 ms segments of a generated 440 Hz sine wave at 48 kHz, stereo, 16-bit PCM amplitude 10000. Each segment contains exactly 4800 frames. They were encoded independently with FFmpeg's `flac` and `libmp3lame` encoders.

They contain no recorded or copyrighted music. FLAC tests check exact sample count and phase continuity; MP3 tests check gapless encoder-delay/padding trimming and seeking.
