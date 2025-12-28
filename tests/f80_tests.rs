use rust_dos::f80::F80;

#[test]
fn test_f80_add_sub_shift_overflow() {
    let mut big_num = F80::new();
    // Set up a large number: Exponent 0x4100 (Arbitrary large exponent)
    // Mantissa: 1.5 (Binary 1.1000...) -> High bit set + next bit
    big_num.set_exponent(0x4100); 
    big_num.set_mantissa(0xC000_0000_0000_0000); // 1.10...

    let mut small_num = F80::new();
    // Set up a small number: Exponent 0x3FFF (Bias, i.e., 1.0)
    // The difference is 0x4100 - 0x3FFF = 0x101 (257 decimal)
    // 257 > 128, so this triggers the "attempt to shift right with overflow" panic.
    small_num.set_exponent(0x3FFF); 
    small_num.set_mantissa(0x8000_0000_0000_0000); // 1.0

    // Test Addition
    let mut res_add = big_num;
    // This will panic if the shift isn't clamped
    res_add.add(small_num);
    
    // Result should effectively be big_num because small_num is infinitesimally small in comparison
    assert_eq!(res_add.get_exponent(), 0x4100);
    assert_eq!(res_add.get_mantissa(), 0xC000_0000_0000_0000);

    // Test Subtraction
    let mut res_sub = big_num;
    // This will panic if the shift isn't clamped
    res_sub.sub(small_num);

    assert_eq!(res_sub.get_exponent(), 0x4100);
    assert_eq!(res_sub.get_mantissa(), 0xC000_0000_0000_0000);
}

#[test]
fn test_f80_extreme_garbage_values() {
    let mut garbage_a = F80::new();
    garbage_a.set_exponent(0x7FFE); // Max valid exponent
    garbage_a.set_mantissa(0xFFFF_FFFF_FFFF_FFFF);

    let mut garbage_b = F80::new();
    garbage_b.set_exponent(1); // Min valid exponent
    garbage_b.set_mantissa(0x8000_0000_0000_0000);

    // Delta is ~32000. Massive shift.
    let mut res = garbage_a;
    res.sub(garbage_b);
    
    // Should not panic, result remains A
    assert_eq!(res.get_exponent(), 0x7FFE);
}