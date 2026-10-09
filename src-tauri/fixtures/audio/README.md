`import.mp3` is a one-second synthetic 440 Hz sine wave, generated locally with
FFmpeg's `sine=frequency=440:duration=1` source and the MP3 encoder. It contains
no recorded speech or personal data. The decoder tests use it to verify MP3
input without requiring an MP3 encoder in the shipped import runtime.
