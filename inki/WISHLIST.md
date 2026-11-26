# Wishlist

## RWKV-Instant and RWKV-Curve schedulers

Neural-network schedulers from [srs-benchmark](https://github.com/open-spaced-repetition/srs-benchmark).
They predict recall more accurately than FSRS-7 (log loss 0.277 / 0.319 vs 0.337),
using the whole review history of every card plus extra features (answer time,
reviews done today, sibling cards, deck structure, day of week).

- **RWKV-Curve** turns each review into a forgetting curve for the card:
  128 mixture weights and 128 residual logits. Recall at any later time is then
  cheap to compute without running the network.
- **RWKV-Instant** predicts recall just before a review. It needs a full network
  pass per card each time it is asked, and it gives no interval.

### Blockers

1. **No licence.** srs-benchmark has no LICENSE file, so its model code and the
   trained weights (`pretrain/RWKV_trained_on_5000_10000.pth`, 11 MB) cannot be
   redistributed. Ask the authors to add one. The RWKV architecture itself
   ([RWKV-LM](https://github.com/BlinkDL/RWKV-LM)) is Apache-2.0.
2. **Too big to run on the device as is.** About 2.76M parameters; the ESP32-C3
   has about 300 KB of free RAM and no FPU. Per-card recurrent state is
   roughly 50-70 KB in fp32, so about 250 MB on the SD card for 5,000 cards.

### Plan once the licence is settled

1. Run the model on the PC (`rwkv/run_as_rnn.py` from srs-benchmark) over the
   full Anki review log, as a new `inki` command.
2. **RWKV-Curve:** write each card's curve (256 floats, ~512 B in fp16) to the
   SD card. The device evaluates it to find due cards; after an on-device review
   it falls back to FSRS-7 for that card until the next PC sync.
3. **RWKV-Instant:** PC only. Use it to rank or pick cards at sync time.
4. Later, maybe: int8 weights in a flash partition and integer inference on the
   device (~0.1 s per review), so curves update right after each review.
