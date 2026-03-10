# Noise Model & Denoising

## Raw Sensor Noise

Total noise variance at signal level x:
```
Var(x) = a × x + b
```
- **a** = shot noise coefficient (Poisson, signal-dependent)
- **b** = read noise variance (Gaussian, signal-independent)
- **x** = signal level (normalized [0, 1])

Standard deviation: `σ(x) = √(max(0, a×x + b))`

This is exactly what the DNG NoiseProfile tag stores.

## Three Noise Regimes

1. **Shadows** (low signal): Read noise dominates. SNR limited by electronics.
2. **Midtones** (medium signal): Shot noise dominates. SNR ∝ √(signal).
3. **Highlights** (high signal): PRNU (pixel response non-uniformity) dominates.

## How Noise Varies with ISO

- Analog gain (g) increases with ISO
- Shot noise 'a' increases ∝ g
- Read noise 'b' increases ∝ g²
- Dynamic range shrinks: white_level / noise_floor decreases
- In electron-referred terms, read noise stays roughly constant

## Estimating Noise from Raw Data

### From DNG NoiseProfile (preferred)
If the DNG has a NoiseProfile tag, use it directly. Per-channel `(S, O)` pairs.

### Wavelet MAD (darktable's approach)
1. Wavelet decomposition of the image
2. Estimate noise from MAD (median absolute deviation) of finest-scale coefficients
3. `σ = MAD / 0.6745` (robust estimator)
4. Fit σ(signal) = √(a×x + b) across brightness levels

### Uniform Patch Method
1. Find uniform regions (low local variance)
2. Compute local mean and local variance across many patches
3. Linear regression: `var = a × mean + b`
4. Slope = shot noise, intercept = read noise

## Relevance to zenraw

### Adaptive Demosaicing
Noise level informs demosaic algorithm selection:
- Low noise (low ISO): Use detail-preserving RCD or AMaZE
- High noise (high ISO): Use noise-suppressing LMMSE
- darktable auto-selects based on estimated noise

### Noise-Gated Sharpening
zenfilters' AdaptiveSharpen uses a `noise_floor` parameter. The DNG NoiseProfile provides the ground truth for this:
```rust
let noise_floor = noise_model.std_dev(channel, signal_level);
// Pass to AdaptiveSharpen to avoid amplifying noise
```

### Denoising Integration
If zenraw exposes the NoiseProfile, downstream consumers (zenfilters or a future denoiser) can use it for:
- Noise-aware bilateral filtering (sigma_range from noise model)
- Wavelet denoising with signal-adaptive thresholds
- Non-local means with noise-dependent search windows

### Example Noise Profiles

Canon 5D Mark II, ISO 3200:
```
a = [4.494e-05, 4.494e-05, 4.494e-05]
b = [-1.063e-06, -1.063e-06, -1.063e-06]
```
(Negative b means calibration noise; clamp to 0.)

A typical modern sensor at ISO 100:
```
a ≈ [1e-06, 1e-06, 1e-06]
b ≈ [1e-08, 1e-08, 1e-08]
```

## Data Structure

```rust
/// Noise model from DNG NoiseProfile: variance = S × signal + O
pub struct NoiseModel {
    /// Shot noise coefficient per channel [R, G, B]
    pub shot_noise: [f64; 3],
    /// Read noise variance per channel [R, G, B]
    pub read_noise: [f64; 3],
}

impl NoiseModel {
    pub fn std_dev(&self, channel: usize, signal: f64) -> f64 {
        (self.shot_noise[channel] * signal + self.read_noise[channel])
            .max(0.0)
            .sqrt()
    }
}
```

## Exposing in RawInfo

zenraw should expose the noise model in `RawInfo` when available:
```rust
pub struct RawInfo {
    // ... existing fields ...
    pub noise_profile: Option<NoiseModel>,
    pub iso: Option<u32>,
    pub baseline_noise: f64,      // relative noise (1.0 = baseline)
    pub baseline_sharpness: f64,  // relative sharpness (1.0 = baseline)
}
```

This lets zenfilters' auto-tune system make noise-aware decisions.
