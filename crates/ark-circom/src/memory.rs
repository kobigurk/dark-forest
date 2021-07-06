//! Safe-ish interface for reading and writing specific types to the WASM runtime's memory
use num_traits::ToPrimitive;
use wasmer::{Memory, MemoryView};

// TODO: Decide whether we want Ark here or if it should use a generic BigInt package
use ark_bn254::FrParameters;
use ark_ff::{BigInteger, BigInteger256, FpParameters, FromBytes, Zero};

use num_bigint::{BigInt, BigUint, Sign};

use color_eyre::Result;
use std::{convert::TryFrom, ops::Deref};

#[derive(Clone, Debug)]
pub struct SafeMem {
    pub memory: Memory,

    short_max: BigInt,
    short_min: BigInt,
    pub prime: BigInt,
    n32: usize,
}

impl Deref for SafeMem {
    type Target = Memory;

    fn deref(&self) -> &Self::Target {
        &self.memory
    }
}

impl SafeMem {
    pub fn new(memory: Memory, n32: usize, prime: BigInt) -> Self {
        let short_max = BigInt::from(0x8000_0000u64);
        let short_min = BigInt::from_biguint(
            num_bigint::Sign::NoSign,
            BigUint::try_from(FrParameters::MODULUS).unwrap(),
        ) - &short_max;

        Self {
            memory,
            short_max,
            short_min,
            prime,
            n32,
        }
    }

    pub fn view(&self) -> MemoryView<u32> {
        self.memory.view()
    }

    pub fn free_pos(&self) -> u32 {
        self.view()[0].get()
    }

    pub fn set_free_pos(&mut self, ptr: u32) {
        self.write_u32(0, ptr);
    }

    pub fn alloc_u32(&mut self) -> u32 {
        let p = self.free_pos();
        self.set_free_pos(p + 8);
        p
    }

    /// Writes a `num` to the provided position of the buffer
    ///
    /// This is marked as `&mut self` for safety
    pub fn write_u32(&mut self, ptr: usize, num: u32) {
        let buf = unsafe { self.memory.data_unchecked_mut() };
        buf[ptr..ptr + std::mem::size_of::<u32>()].copy_from_slice(&num.to_le_bytes());
    }

    /// Reads a u32 from the specific slice
    pub fn read_u32(&self, ptr: usize) -> u32 {
        let buf = unsafe { self.memory.data_unchecked() };

        let mut bytes = [0; 4];
        bytes.copy_from_slice(&buf[ptr..ptr + std::mem::size_of::<u32>()]);

        u32::from_le_bytes(bytes)
    }

    pub fn alloc_fr(&mut self) -> u32 {
        let n32 = 8;
        let p = self.free_pos();
        self.set_free_pos(p + n32 * 4 + 8);
        p
    }

    pub fn write_fr(&mut self, ptr: usize, fr: &BigInt) -> Result<()> {
        if fr < &self.short_max && fr > &self.short_min {
            if fr >= &BigInt::zero() {
                self.write_short_positive(ptr, fr)?;
            } else {
                self.write_short_negative(ptr, fr)?;
            }
        } else {
            self.write_long_normal(ptr, fr)?;
        }

        Ok(())
    }

    // https://github.com/iden3/go-circom-witnesscalc/blob/25592ab9b33bf8d6b99c133783bd208bee7a935c/witnesscalc.go#L410-L430
    // TODO: Figure out WTF all this parsing is for
    pub fn read_fr(&self, ptr: usize) -> Result<BigInt> {
        let view = self.memory.view::<u32>();

        let res = if view[ptr + 1].get() & 0x80000000 != 0 {
            let num = self.read_big(ptr + 8, self.n32)?;
            num
        } else {
            // read the number
            let mut res = self.read_big(ptr, 4).unwrap();

            // adjust the sign if negative
            if view[ptr].get() & 0x80000000 != 0 {
                res -= BigInt::from(0x100000000i64)
            }
            res
        };

        Ok(res)
    }

    fn write_short_positive(&mut self, ptr: usize, fr: &BigInt) -> Result<()> {
        let num = fr.to_i32().expect("not a short positive");
        self.write_u32(ptr, num as u32);
        self.write_u32(ptr + 4, 0);
        Ok(())
    }

    fn write_short_negative(&mut self, ptr: usize, fr: &BigInt) -> Result<()> {
        let num = fr - &self.short_min;
        let num = num - &self.short_max;
        let num = num + BigInt::from(0x0001_0000_0000i64);

        let num = num
            .to_u32()
            .expect("could not cast as u32 (should never happen)");

        self.write_u32(ptr, num);
        self.write_u32(ptr + 4, 0);
        Ok(())
    }

    fn write_long_normal(&mut self, ptr: usize, fr: &BigInt) -> Result<()> {
        self.write_u32(ptr, 0);
        self.write_u32(ptr + 4, i32::MIN as u32); // 0x80000000
        self.write_big(ptr + 8, fr)?;
        Ok(())
    }

    fn write_big(&self, ptr: usize, num: &BigInt) -> Result<()> {
        let buf = unsafe { self.memory.data_unchecked_mut() };

        // always positive?
        let (_, num) = num.clone().into_parts();
        let num = BigInteger256::try_from(num).unwrap();

        let bytes = num.to_bytes_le();
        let len = bytes.len();
        buf[ptr..ptr + len].copy_from_slice(&bytes);

        Ok(())
    }

    pub fn read_big(&self, ptr: usize, num_bytes: usize) -> Result<BigInt> {
        let buf = unsafe { self.memory.data_unchecked() };
        let buf = &buf[ptr..ptr + num_bytes * 32];

        // TODO: Is there a better way to read big integers?
        let big = BigInteger256::read(buf).unwrap();
        dbg!(&big);
        let big = BigUint::try_from(big).unwrap();
        Ok(big.into())
    }
}

// TODO: Figure out how to read / write numbers > u32
// circom-witness-calculator: Wasm + Memory -> expose BigInts so that they can be consumed by any proof system
// ark-circom:
// 1. can read zkey
// 2. can generate witness from inputs
// 3. can generate proofs
// 4. can serialize proofs in the desired format
#[cfg(test)]
mod tests {
    use super::*;
    use num_traits::ToPrimitive;
    use std::str::FromStr;
    use wasmer::{MemoryType, Store};

    fn new() -> SafeMem {
        SafeMem::new(
            Memory::new(&Store::default(), MemoryType::new(1, None, false)).unwrap(),
            2,
            BigInt::from_str(
                "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            )
            .unwrap(),
        )
    }

    #[test]
    fn i32_bounds() {
        let mem = new();
        let i32_max = i32::MAX as i64 + 1;
        assert_eq!(mem.short_min.to_i64().unwrap(), -i32_max);
        assert_eq!(mem.short_max.to_i64().unwrap(), i32_max);
    }

    #[test]
    fn read_write_32() {
        let mut mem = new();
        let num = u32::MAX;

        let inp = mem.read_u32(0);
        assert_eq!(inp, 0);

        mem.write_u32(0, num);
        let inp = mem.read_u32(0);
        assert_eq!(inp, num);
    }

    #[test]
    fn read_write_fr_small_positive() {
        read_write_fr(BigInt::from(1_000_000), BigInt::from(1_000_000));
    }

    #[test]
    fn read_write_fr_small_negative() {
        read_write_fr(
            BigInt::from(-1_000_000),
            BigInt::from(-1_000_000),
        );
    }

    #[test]
    fn read_write_fr_big_positive() {
        read_write_fr(BigInt::from(500000000000i64), BigInt::from(500000000000i64));
    }

    // TODO: How should this be handled?
    #[test]
    fn read_write_fr_big_negative() {
        read_write_fr(
            BigInt::from_str("-500000000000").unwrap(),
            BigInt::from_str("-500000000000").unwrap(),
            // "21888242871839275222246405745257275088548364400416034343698204186574024701953"
            //     .parse()
            //     .unwrap(),
        )
    }

    fn read_write_fr(num: BigInt, expected: BigInt) {
        let mut mem = new();
        mem.write_fr(0, &num).unwrap();
        let res = mem.read_fr(0).unwrap();
        assert_eq!(res, expected);
    }
}
