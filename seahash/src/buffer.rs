//! A highly optimized version of SeaHash.

use core::slice;

use diffuse;

/// Read a buffer smaller than 8 bytes into an integer in little-endian.
///
/// This assumes that `buf.len() < 8`. If this is not satisfied, the behavior is unspecified.
#[inline(always)]
fn read_int(buf: &[u8]) -> u64 {
    // Because we want to make sure that it is register allocated, we fetch this into a variable.
    // It will likely make no difference anyway, though.
    let ptr = buf.as_ptr();

    unsafe {
        // Break it down to reads of integers with widths in total spanning the buffer. This minimizes
        // the number of reads
        match buf.len() {
            // u8.
            1 => *ptr as u64,
            // u16.
            2 => (*(ptr as *const u16)).to_le() as u64,
            // u16 + u8.
            3 => {
                let a = (*(ptr as *const u16)).to_le() as u64;
                let b = *ptr.offset(2) as u64;

                a | (b << 16)
            },
            // u32.
            4 => (*(ptr as *const u32)).to_le() as u64,
            // u32 + u8.
            5 => {
                let a = (*(ptr as *const u32)).to_le() as u64;
                let b = *ptr.offset(4) as u64;

                a | (b << 32)
            },
            // u32 + u16.
            6 => {
                let a = (*(ptr as *const u32)).to_le() as u64;
                let b = (*(ptr.offset(4) as *const u16)).to_le() as u64;

                a | (b << 32)
            },
            // u32 + u16 + u8.
            7 => {
                let a = (*(ptr as *const u32)).to_le() as u64;
                let b = (*(ptr.offset(4) as *const u16)).to_le() as u64;
                let c = *ptr.offset(6) as u64;

                a | (b << 32) | (c << 48)
            },
            _ => 0,
        }
    }
}

/// Read a little-endian 64-bit integer from some buffer.
#[inline(always)]
unsafe fn read_u64(ptr: *const u8) -> u64 {
    #[cfg(target_pointer_width = "32")]
    {
        (*(ptr as *const u32)).to_le() as u64 | ((*(ptr as *const u32)).to_le() as u64) << 32
    }

    #[cfg(target_pointer_width = "64")]
    {
        (*(ptr as *const u64)).to_le()
    }
}

/// Hash some buffer.
///
/// This is a highly optimized implementation of SeaHash. It implements numerous techniques to
/// improve performance:
///
/// - Register allocation: This makes a great deal out of making sure everything fits into
///   registers such that minimal memory accesses are needed. This works quite successfully on most
///   CPUs, and the only time it reads from memory is when it fetches the data of the buffer.
/// - SIMD reads: Like most other good hash functions, we read 8 bytes a time. This improves things
///   a lot
/// - Independent updates: We make sure very few statements next to each other depends on the
///   other. This means that almost always the CPU will be able to run the instructions in parallel.
/// - Loop unrolling: The hot loop is unrolled such that very little branches (one every 32 bytes)
///   are needed.
///
/// and more.
pub fn hash(buf: &[u8]) -> u64 {
    unsafe {
        // We use 4 different registers to store seperate hash states, because this allows us to update
        // them seperately, and consequently exploiting ILP to update the states in parallel.
        let mut a = 0x16f11fe89b0d677c;
        let mut b = 0xb480a793d8e6c86c;
        let mut c = 0x6fe2e5aaf078ebc9;
        let mut d = 0x14f994a4c5259381;

        // The pointer to the current bytes.
        let mut ptr = buf.as_ptr();
        /// The end of the "main segment", i.e. the biggest buffer s.t. the length is divisible by
        /// 32.
        let end_ptr = buf.as_ptr().offset(buf.len() as isize & !0x1F) as usize;

        while end_ptr >= ptr as usize {
            // Read and diffuse the next 4 64-bit little-endian integers from their bytes. Note
            // that we on purpose not use `^=` and co., because it aliases the lvalue, making it
            // harder for LLVM to register allocate (it will have to inline the value behind the
            // pointer, effectively assuming that it is not aliased, which can be hard to prove).

            // Placing these updates inplace can have some negative consequences on especially
            // older architectures, where they can block ILP because they assume the evaluation of
            // the old `byte` is executed, which might trigger the diffusion to run serially.
            // However, not introducing a tmp register makes sure that you don't push from the
            // register to the stack, which comes with a performance hit.
            a = a ^ read_u64(ptr);
            ptr = ptr.offset(8);

            b = b ^ read_u64(ptr);
            ptr = ptr.offset(8);

            c = c ^ read_u64(ptr);
            ptr = ptr.offset(8);

            d = d ^ read_u64(ptr);
            ptr = ptr.offset(8);

            // Diffuse the updated registers. We hope that each of these are executed in parallel.
            a = diffuse(a);
            b = diffuse(b);
            c = diffuse(c);
            d = diffuse(d);
        }

        // Rename the register (we do this to make it easier for LLVM to reallocate the register).
        let mut excessive = end_ptr;
        // Calculate the number of excessive bytes. These are bytes that could not be handled in
        // the loop above.
        excessive = buf.len() as usize + buf.as_ptr() as usize - excessive as usize;
        // Handle the excessive bytes.
        match excessive {
            0 => {},
            1...7 => {
                // 1 or more excessive.

                // Write the last excessive bytes (<8 bytes).
                a = a ^ read_int(slice::from_raw_parts(ptr as *const u8, excessive));

                // Diffuse.
                a = diffuse(a);
            },
            8 => {
                // 8 bytes excessive.

                // Update `a`.
                a = a ^ read_u64(ptr);

                // Diffuse.
                a = diffuse(a);
            },
            9...15 => {
                // More than 8 bytes excessive.

                // Update `a`.
                a = a ^ read_u64(ptr);
                ptr = ptr.offset(8);

                // Write the last excessive bytes (<8 bytes).
                excessive = excessive - 8;
                b = b ^ read_int(slice::from_raw_parts(ptr as *const u8, excessive));

                // Diffuse.
                a = diffuse(a);
                b = diffuse(b);

            },
            16 => {
                // 16 bytes excessive.

                // Update `a`.
                a = a ^ read_u64(ptr);
                ptr = ptr.offset(8);
                // Update `b`.
                b = b ^ read_u64(ptr);

                // Diffuse.
                a = diffuse(a);
                b = diffuse(b);
            },
            17...23 => {
                // 16 bytes or more excessive.

                // Update `a`.
                a = a ^ read_u64(ptr);
                ptr = ptr.offset(8);
                // Update `b`.
                b = b ^ read_u64(ptr);

                // Write the last excessive bytes (<8 bytes).
                excessive = excessive - 16;
                c = c ^ read_int(slice::from_raw_parts(ptr as *const u8, excessive));

                // Diffuse.
                a = diffuse(a);
                b = diffuse(b);
                c = diffuse(c);
            },
            24 => {
                // 24 bytes excessive.

                // Update `a`.
                a = a ^ read_u64(ptr);
                ptr = ptr.offset(8);
                // Update `b`.
                b = b ^ read_u64(ptr);

                // Diffuse.
                a = diffuse(a);
                b = diffuse(b);
            },
            _ => {
                // More than 24 bytes excessive.

                // Update `a`.
                a = a ^ read_u64(ptr);
                ptr = ptr.offset(8);
                // Update `b`.
                b = b ^ read_u64(ptr);
                ptr = ptr.offset(8);
                // Update `c`.
                c = c ^ read_u64(ptr);
                ptr = ptr.offset(8);

                // Write the last excessive bytes (<8 bytes).
                excessive = excessive - 24;
                d = d ^ read_int(slice::from_raw_parts(ptr as *const u8, excessive));

                // Diffuse.
                a = diffuse(a);
                b = diffuse(b);
                c = diffuse(c);
                d = diffuse(d);

            }
        }

        // XOR the states together. Even though XOR is commutative, it doesn't matter, because the
        // state vector's initial components are mutually distinct, and thus swapping even and odd
        // chunks will affect the result, because it is sensitive to the initial condition.
        a = a ^ b;
        c = c ^ d;
        a = a ^ c;
        // XOR the number of written bytes in order to make the excessive bytes zero-sensitive
        // (without this, two excessive zeros would be equivalent to three excessive zeros). This
        // is know as length padding.
        a = a ^ buf.len() as u64;

        // We diffuse to make the excessive bytes discrete (i.e. small changes shouldn't give small
        // changes in the output).
        diffuse(a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use reference;

    fn hash_match(a: &[u8]) {
        assert_eq!(hash(a), reference::hash(a));
    }

    #[test]
    fn zero() {
        let arr = [0; 4096];
        for n in 0..4096 {
            hash_match(&arr[0..n]);
        }
    }

    #[test]
    fn seq() {
        let mut buf = [0; 4096];
        for i in 0..4096 {
            buf[i] = i as u8;
        }
        hash_match(&buf);
    }


    #[test]
    fn position_depedent() {
        let mut buf1 = [0; 4098];
        for i in 0..4098 {
            buf1[i] = i as u8;
        }
        let mut buf2 = [0; 4098];
        for i in 0..4098 {
            buf2[i] = i as u8 ^ 1;
        }

        assert!(hash(&buf1) != hash(&buf2));
    }

    #[test]
    fn shakespear() {
        hash_match(b"to be or not to be");
        hash_match(b"love is a wonderful terrible thing");
    }

    #[test]
    fn zero_senitive() {
        assert_ne!(hash(&[1, 2, 3, 4]), hash(&[1, 0, 2, 3, 4]));
        assert_ne!(hash(&[1, 2, 3, 4]), hash(&[1, 0, 0, 2, 3, 4]));
        assert_ne!(hash(&[1, 2, 3, 4]), hash(&[1, 2, 3, 4, 0]));
        assert_ne!(hash(&[1, 2, 3, 4]), hash(&[0, 1, 2, 3, 4]));
        assert_ne!(hash(&[0, 0, 0]), hash(&[0, 0, 0, 0, 0]));
    }

    #[test]
    fn not_equal() {
        assert_ne!(hash(b"to be or not to be "), hash(b"to be or not to be"));
        assert_ne!(hash(b"jkjke"), hash(b"jkjk"));
        assert_ne!(hash(b"ijkjke"), hash(b"ijkjk"));
        assert_ne!(hash(b"iijkjke"), hash(b"iijkjk"));
        assert_ne!(hash(b"iiijkjke"), hash(b"iiijkjk"));
        assert_ne!(hash(b"iiiijkjke"), hash(b"iiiijkjk"));
        assert_ne!(hash(b"iiiiijkjke"), hash(b"iiiiijkjk"));
        assert_ne!(hash(b"iiiiiijkjke"), hash(b"iiiiiijkjk"));
        assert_ne!(hash(b"iiiiiiijkjke"), hash(b"iiiiiiijkjk"));
        assert_ne!(hash(b"iiiiiiiijkjke"), hash(b"iiiiiiiijkjk"));
        assert_ne!(hash(b"ab"), hash(b"bb"));
    }
}