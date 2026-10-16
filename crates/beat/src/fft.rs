//! Minimal power-of-two complex FFT - the one piece of numpy's
//! `np.fft.rfft` that `onset_envelope` needs, kept in-tree so the crate
//! stays dependency-free.

/// In-place iterative radix-2 Cooley-Tukey FFT of the complex signal
/// `(re, im)`. Length must be a power of two (and at least 1).
///
/// Unnormalized, like `numpy.fft.fft`: forward transforms scale by `n`,
/// so pair with a `1/n` factor when mirroring `numpy.fft.irfft`.
pub(crate) fn fft(re: &mut [f64], im: &mut [f64]) {
    debug_assert_eq!(re.len(), im.len());
    let n = re.len();
    debug_assert!(n.is_power_of_two());

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // Danielson-Lanczos butterflies, doubling the sub-DFT length each pass.
    let mut len = 2usize;
    while len <= n {
        let angle = -2.0 * std::f64::consts::PI / len as f64;
        let (w_re, w_im) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cur_re, mut cur_im) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (u_re, u_im) = (re[start + k], im[start + k]);
                let (x_re, x_im) = (re[start + k + len / 2], im[start + k + len / 2]);
                let v_re = x_re * cur_re - x_im * cur_im;
                let v_im = x_re * cur_im + x_im * cur_re;
                re[start + k] = u_re + v_re;
                im[start + k] = u_im + v_im;
                re[start + k + len / 2] = u_re - v_re;
                im[start + k + len / 2] = u_im - v_im;
                let (next_re, next_im) =
                    (cur_re * w_re - cur_im * w_im, cur_re * w_im + cur_im * w_re);
                cur_re = next_re;
                cur_im = next_im;
            }
        }
        len *= 2;
    }
}
