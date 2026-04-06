//! CPU-based tensor math operations for transformer inference.
//!
//! All operations work on contiguous f32 slices in row-major order.
//! Designed for bare-metal LLM inference on ClaudioOS.

use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Core Tensor type
// ---------------------------------------------------------------------------

/// A dense tensor backed by a contiguous `Vec<f32>` in row-major order.
pub struct Tensor {
    pub data: Vec<f32>,
    pub shape: Vec<usize>,
}

impl Tensor {
    /// Create a tensor filled with zeros.
    pub fn zeros(shape: &[usize]) -> Self {
        let n: usize = shape.iter().product();
        Self {
            data: vec![0.0f32; n],
            shape: shape.to_vec(),
        }
    }

    /// Create a tensor from an f32 slice with the given shape.
    ///
    /// # Panics
    /// Panics if `data.len()` does not match the product of `shape`.
    pub fn from_f32_slice(data: &[f32], shape: &[usize]) -> Self {
        let n: usize = shape.iter().product();
        assert_eq!(data.len(), n, "data length {} != shape product {}", data.len(), n);
        Self {
            data: data.to_vec(),
            shape: shape.to_vec(),
        }
    }

    /// Total number of elements in the tensor.
    pub fn numel(&self) -> usize {
        self.shape.iter().product()
    }

    /// Reshape the tensor in-place.
    ///
    /// # Panics
    /// Panics if the new shape has a different total element count.
    pub fn reshape(&mut self, shape: &[usize]) {
        let n: usize = shape.iter().product();
        assert_eq!(
            self.data.len(),
            n,
            "reshape: current {} elements != new shape {} elements",
            self.data.len(),
            n
        );
        self.shape = shape.to_vec();
    }
}

// ---------------------------------------------------------------------------
// Matrix operations
// ---------------------------------------------------------------------------

/// Matrix multiply: `(M, K) x (K, N) -> (M, N)`, row-major.
///
/// `out` must have length `m * n`, `a` length `m * k`, `b` length `k * n`.
pub fn matmul(out: &mut [f32], a: &[f32], b: &[f32], m: usize, k: usize, n: usize) {
    debug_assert_eq!(out.len(), m * n);
    debug_assert_eq!(a.len(), m * k);
    debug_assert_eq!(b.len(), k * n);

    for i in 0..m {
        for j in 0..n {
            let mut sum = 0.0f32;
            for p in 0..k {
                sum += a[i * k + p] * b[p * n + j];
            }
            out[i * n + j] = sum;
        }
    }
}

/// Matrix-vector multiply: `(M, K) x (K,) -> (M,)`, row-major.
///
/// Common case during inference with batch size 1.
pub fn matvec(out: &mut [f32], mat: &[f32], v: &[f32], m: usize, k: usize) {
    debug_assert_eq!(out.len(), m);
    debug_assert_eq!(mat.len(), m * k);
    debug_assert_eq!(v.len(), k);

    for i in 0..m {
        let mut sum = 0.0f32;
        for j in 0..k {
            sum += mat[i * k + j] * v[j];
        }
        out[i] = sum;
    }
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

/// Root Mean Square Layer Normalization.
///
/// `out[i] = (x[i] / sqrt(mean(x^2) + eps)) * weight[i]`
pub fn rmsnorm(out: &mut [f32], x: &[f32], weight: &[f32], n: usize, eps: f32) {
    debug_assert_eq!(out.len(), n);
    debug_assert_eq!(x.len(), n);
    debug_assert_eq!(weight.len(), n);

    // Compute 1/RMS as a single scale factor to avoid per-element division
    let mut ss = 0.0f32;
    for i in 0..n {
        ss += x[i] * x[i];
    }
    ss = 1.0 / libm::sqrtf(ss / n as f32 + eps);

    for i in 0..n {
        out[i] = x[i] * ss * weight[i];
    }
}

// ---------------------------------------------------------------------------
// Activation functions
// ---------------------------------------------------------------------------

/// Numerically stable softmax in-place.
///
/// Subtracts the max value before exponentiating to avoid overflow.
pub fn softmax(x: &mut [f32]) {
    if x.is_empty() {
        return;
    }

    // Find max for numerical stability
    let mut max_val = x[0];
    for &v in x.iter().skip(1) {
        if v > max_val {
            max_val = v;
        }
    }

    // Exponentiate and sum
    let mut sum = 0.0f32;
    for v in x.iter_mut() {
        *v = libm::expf(*v - max_val);
        sum += *v;
    }

    // Normalize
    let inv_sum = 1.0 / sum;
    for v in x.iter_mut() {
        *v *= inv_sum;
    }
}

/// SiLU (Sigmoid Linear Unit) / Swish activation, in-place.
///
/// `x[i] = x[i] * sigmoid(x[i])` = `x[i] / (1 + exp(-x[i]))`
pub fn silu(x: &mut [f32]) {
    for v in x.iter_mut() {
        *v = *v / (1.0 + libm::expf(-*v));
    }
}

// ---------------------------------------------------------------------------
// Positional encoding
// ---------------------------------------------------------------------------

/// Rotary Position Embeddings (RoPE).
///
/// For each head, pairs `(q[2i], q[2i+1])` are rotated by angle
/// `pos * freq_i` where `freq_i = 1 / (theta ^ (2i / head_dim))`.
/// Same rotation is applied to the corresponding `k` pairs.
pub fn rope(
    q: &mut [f32],
    k: &mut [f32],
    head_dim: usize,
    pos: usize,
    n_heads: usize,
    rope_theta: f32,
) {
    // RoPE operates on consecutive pairs, so we iterate over half the dimension
    let half_dim = head_dim / 2;

    for h in 0..n_heads {
        let q_off = h * head_dim;
        let k_off = h * head_dim;

        for i in 0..half_dim {
            // Frequency for dimension pair i: theta^(-2i/d) gives lower frequencies
            // for later dimensions, encoding coarser positional info
            let freq = 1.0 / libm::powf(rope_theta, (2 * i) as f32 / head_dim as f32);
            let angle = pos as f32 * freq;
            let cos_a = libm::cosf(angle);
            let sin_a = libm::sinf(angle);

            // Rotate q pair by angle using 2D rotation matrix: [cos -sin; sin cos]
            if q_off + 2 * i + 1 < q.len() {
                let q0 = q[q_off + 2 * i];
                let q1 = q[q_off + 2 * i + 1];
                q[q_off + 2 * i] = q0 * cos_a - q1 * sin_a;
                q[q_off + 2 * i + 1] = q0 * sin_a + q1 * cos_a;
            }

            // Rotate k
            if k_off + 2 * i + 1 < k.len() {
                let k0 = k[k_off + 2 * i];
                let k1 = k[k_off + 2 * i + 1];
                k[k_off + 2 * i] = k0 * cos_a - k1 * sin_a;
                k[k_off + 2 * i + 1] = k0 * sin_a + k1 * cos_a;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Element-wise operations
// ---------------------------------------------------------------------------

/// Element-wise multiply: `out[i] *= a[i]`.
pub fn elementwise_mul(out: &mut [f32], a: &[f32]) {
    debug_assert_eq!(out.len(), a.len());
    for i in 0..out.len() {
        out[i] *= a[i];
    }
}

/// Element-wise add: `out[i] += a[i]`.
pub fn add(out: &mut [f32], a: &[f32]) {
    debug_assert_eq!(out.len(), a.len());
    for i in 0..out.len() {
        out[i] += a[i];
    }
}

/// Copy slice: `dst[i] = src[i]`.
pub fn copy(dst: &mut [f32], src: &[f32]) {
    debug_assert_eq!(dst.len(), src.len());
    dst.copy_from_slice(src);
}

/// Index of the maximum value in the slice.
///
/// Returns 0 for an empty slice.
pub fn argmax(x: &[f32]) -> usize {
    if x.is_empty() {
        return 0;
    }
    let mut max_idx = 0usize;
    let mut max_val = x[0];
    for (i, &v) in x.iter().enumerate().skip(1) {
        if v > max_val {
            max_val = v;
            max_idx = i;
        }
    }
    max_idx
}

// ---------------------------------------------------------------------------
// Dequantization
// ---------------------------------------------------------------------------

/// Convert IEEE 754 half-precision (f16) to f32.
pub fn f16_to_f32(h: u16) -> f32 {
    // Extract f16 fields: 1 sign bit, 5 exponent bits, 10 mantissa bits
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let mant = (h & 0x3FF) as u32;

    if exp == 0 {
        if mant == 0 {
            // Signed zero: preserve sign bit only
            return f32::from_bits(sign << 31);
        }
        // Subnormal f16 -> normal f32: shift mantissa up until the implicit 1 bit appears
        let mut e = 0u32;
        let mut m = mant;
        while (m & 0x400) == 0 {
            m <<= 1;
            e += 1;
        }
        // Re-bias exponent from f16 bias (15) to f32 bias (127), minus normalization shifts
        let exp32 = 127 - 15 - e;
        // Shift 10-bit mantissa to fill the f32 23-bit mantissa field
        let mant32 = (m & 0x3FF) << 13;
        return f32::from_bits((sign << 31) | (exp32 << 23) | mant32);
    }
    if exp == 31 {
        // Inf or NaN: map to f32 all-ones exponent; use quiet NaN if mantissa nonzero
        let mant32 = if mant != 0 { 0x7FC000 } else { 0 };
        return f32::from_bits((sign << 31) | 0x7F800000 | mant32);
    }

    // Normal f16: re-bias exponent and widen mantissa
    let exp32 = exp + 127 - 15;
    let mant32 = mant << 13;
    f32::from_bits((sign << 31) | (exp32 << 23) | mant32)
}

/// Dequantize Q4_0 blocks to f32.
///
/// Q4_0 block layout (32 elements per block):
///   - 2 bytes: scale as f16
///   - 16 bytes: 32 nibbles (4-bit signed integers, packed two per byte)
///
/// Each nibble is interpreted as an unsigned 0..15 value, then shifted by -8
/// to get a signed range of -8..7, then multiplied by the block scale.
pub fn dequantize_q4_0(out: &mut [f32], data: &[u8], n_elements: usize) {
    const BLOCK_SIZE: usize = 32;
    const BLOCK_BYTES: usize = 2 + 16; // f16 scale + 16 bytes of nibbles

    let n_blocks = n_elements / BLOCK_SIZE;

    for block in 0..n_blocks {
        let block_data = &data[block * BLOCK_BYTES..];

        // First 2 bytes: f16 scale factor
        let scale_bits = u16::from_le_bytes([block_data[0], block_data[1]]);
        let scale = f16_to_f32(scale_bits);

        // Next 16 bytes: 32 nibbles packed two per byte (lo=bits[3:0], hi=bits[7:4])
        for i in 0..16 {
            let byte = block_data[2 + i];
            // Unsigned nibble [0,15] shifted to signed range [-8,7]
            let lo = (byte & 0x0F) as i32 - 8;
            let hi = ((byte >> 4) & 0x0F) as i32 - 8;

            let out_idx = block * BLOCK_SIZE + i * 2;
            if out_idx < n_elements {
                out[out_idx] = lo as f32 * scale;
            }
            if out_idx + 1 < n_elements {
                out[out_idx + 1] = hi as f32 * scale;
            }
        }
    }
}

/// Dequantize Q8_0 blocks to f32.
///
/// Q8_0 block layout (32 elements per block):
///   - 2 bytes: scale as f16
///   - 32 bytes: 32 signed int8 values
pub fn dequantize_q8_0(out: &mut [f32], data: &[u8], n_elements: usize) {
    const BLOCK_SIZE: usize = 32;
    const BLOCK_BYTES: usize = 2 + 32; // f16 scale + 32 int8s

    let n_blocks = n_elements / BLOCK_SIZE;

    for block in 0..n_blocks {
        let block_data = &data[block * BLOCK_BYTES..];

        // First 2 bytes: f16 scale factor
        let scale_bits = u16::from_le_bytes([block_data[0], block_data[1]]);
        let scale = f16_to_f32(scale_bits);

        // Next 32 bytes: signed int8 values
        for i in 0..32 {
            let val = block_data[2 + i] as i8;
            let out_idx = block * BLOCK_SIZE + i;
            if out_idx < n_elements {
                out[out_idx] = val as f32 * scale;
            }
        }
    }
}

/// Dequantize f16 values to f32.
///
/// Each element is 2 bytes (IEEE 754 half precision, little-endian).
pub fn dequantize_f16(out: &mut [f32], data: &[u8], n_elements: usize) {
    for i in 0..n_elements {
        let offset = i * 2;
        let bits = u16::from_le_bytes([data[offset], data[offset + 1]]);
        out[i] = f16_to_f32(bits);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tensor_zeros() {
        let t = Tensor::zeros(&[2, 3]);
        assert_eq!(t.numel(), 6);
        assert_eq!(t.data.len(), 6);
        assert!(t.data.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_tensor_from_slice() {
        let data = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let t = Tensor::from_f32_slice(&data, &[2, 3]);
        assert_eq!(t.numel(), 6);
        assert_eq!(t.data[4], 5.0);
    }

    #[test]
    fn test_tensor_reshape() {
        let mut t = Tensor::zeros(&[2, 3]);
        t.reshape(&[3, 2]);
        assert_eq!(t.shape, vec![3, 2]);
    }

    #[test]
    #[should_panic]
    fn test_tensor_reshape_mismatch() {
        let mut t = Tensor::zeros(&[2, 3]);
        t.reshape(&[4, 4]);
    }

    #[test]
    fn test_matmul_identity() {
        // 2x2 identity * [1,2; 3,4]
        let a = [1.0, 0.0, 0.0, 1.0];
        let b = [1.0, 2.0, 3.0, 4.0];
        let mut out = [0.0f32; 4];
        matmul(&mut out, &a, &b, 2, 2, 2);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_matmul_2x3_3x2() {
        let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // 2x3
        let b = [7.0, 8.0, 9.0, 10.0, 11.0, 12.0]; // 3x2
        let mut out = [0.0f32; 4]; // 2x2
        matmul(&mut out, &a, &b, 2, 3, 2);
        // [1*7+2*9+3*11, 1*8+2*10+3*12, 4*7+5*9+6*11, 4*8+5*10+6*12]
        assert_eq!(out, [58.0, 64.0, 139.0, 154.0]);
    }

    #[test]
    fn test_matvec() {
        let mat = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // 2x3
        let v = [1.0, 2.0, 3.0];
        let mut out = [0.0f32; 2];
        matvec(&mut out, &mat, &v, 2, 3);
        assert_eq!(out, [14.0, 32.0]);
    }

    #[test]
    fn test_softmax() {
        let mut x = [1.0, 2.0, 3.0];
        softmax(&mut x);
        let sum: f32 = x.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
        assert!(x[2] > x[1] && x[1] > x[0]);
    }

    #[test]
    fn test_softmax_large_values() {
        // Should not overflow thanks to max subtraction
        let mut x = [1000.0, 1001.0, 1002.0];
        softmax(&mut x);
        let sum: f32 = x.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_rmsnorm() {
        let x = [1.0, 2.0, 3.0, 4.0];
        let w = [1.0, 1.0, 1.0, 1.0];
        let mut out = [0.0f32; 4];
        rmsnorm(&mut out, &x, &w, 4, 1e-5);
        // rms = sqrt((1+4+9+16)/4) = sqrt(7.5) ~= 2.7386
        // out[i] = x[i] / rms
        let rms = libm::sqrtf(7.5 + 1e-5);
        for i in 0..4 {
            assert!((out[i] - x[i] / rms).abs() < 1e-4);
        }
    }

    #[test]
    fn test_silu() {
        let mut x = [0.0, 1.0, -1.0];
        silu(&mut x);
        // silu(0) = 0, silu(1) = 1/(1+e^-1) ~= 0.7311, silu(-1) = -1/(1+e^1) ~= -0.2689
        assert!((x[0] - 0.0).abs() < 1e-5);
        assert!((x[1] - 0.7310586).abs() < 1e-4);
        assert!((x[2] - (-0.26894143)).abs() < 1e-4);
    }

    #[test]
    fn test_elementwise_mul() {
        let mut out = [1.0, 2.0, 3.0];
        let a = [4.0, 5.0, 6.0];
        elementwise_mul(&mut out, &a);
        assert_eq!(out, [4.0, 10.0, 18.0]);
    }

    #[test]
    fn test_add() {
        let mut out = [1.0, 2.0, 3.0];
        let a = [4.0, 5.0, 6.0];
        add(&mut out, &a);
        assert_eq!(out, [5.0, 7.0, 9.0]);
    }

    #[test]
    fn test_copy() {
        let src = [1.0, 2.0, 3.0];
        let mut dst = [0.0f32; 3];
        copy(&mut dst, &src);
        assert_eq!(dst, src);
    }

    #[test]
    fn test_argmax() {
        assert_eq!(argmax(&[1.0, 3.0, 2.0]), 1);
        assert_eq!(argmax(&[5.0, 1.0, 2.0]), 0);
        assert_eq!(argmax(&[1.0, 2.0, 5.0]), 2);
        assert_eq!(argmax(&[]), 0);
    }

    #[test]
    fn test_f16_to_f32_basic() {
        // f16 for 1.0: sign=0, exp=15, mant=0 -> 0 01111 0000000000 -> 0x3C00
        let val = f16_to_f32(0x3C00);
        assert!((val - 1.0).abs() < 1e-6);

        // f16 for -1.0: 0xBC00
        let val = f16_to_f32(0xBC00);
        assert!((val - (-1.0)).abs() < 1e-6);

        // f16 for 0.0: 0x0000
        let val = f16_to_f32(0x0000);
        assert_eq!(val, 0.0);

        // f16 for infinity: 0x7C00
        let val = f16_to_f32(0x7C00);
        assert!(val.is_infinite() && val > 0.0);
    }

    #[test]
    fn test_dequantize_f16() {
        // Two f16 values: 1.0 (0x3C00) and -1.0 (0xBC00)
        let data = [0x00, 0x3C, 0x00, 0xBC]; // little-endian
        let mut out = [0.0f32; 2];
        dequantize_f16(&mut out, &data, 2);
        assert!((out[0] - 1.0).abs() < 1e-6);
        assert!((out[1] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_rope_basic() {
        // Just verify it runs without panic and modifies values
        let mut q = [1.0, 0.0, 1.0, 0.0];
        let mut k = [1.0, 0.0, 1.0, 0.0];
        rope(&mut q, &mut k, 4, 1, 1, 10000.0);
        // After rotation at pos=1, values should be different from input
        assert!((q[0] - 1.0).abs() > 1e-6 || (q[1] - 0.0).abs() > 1e-6);
    }
}
