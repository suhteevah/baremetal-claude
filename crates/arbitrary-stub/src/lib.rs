#![no_std]
// Stub for the arbitrary crate — provides empty trait so dependents compile
pub trait Arbitrary<'a>: Sized {
    fn arbitrary(_: &mut Unstructured<'a>) -> Result<Self, Error> { unimplemented!() }
}
pub struct Unstructured<'a>(&'a [u8]);
pub struct Error;
pub use core::result::Result;
