//! Backing-buffer pressure supplements gc-arena's allocation-debt pacing.
use gc_arena::Mutation;

// These are scheduling weights, not allocation limits or estimates of GC wall time.
// Tiny buffers rely on ordinary object debt; 4 KiB of backing storage adds one unit.
const BYTES_PER_UNIT: usize = 4096;
const QUANTUM_DEBT: u32 = 256;
const DEBT_CHECK_INTERVAL: usize = 32;

pub fn charge(mc: &Mutation<'_>, bytes: usize) {
    let units = u32::try_from(bytes / BYTES_PER_UNIT).unwrap_or(u32::MAX);
    mc.metrics().adjust_debt(f64::from(units));
}

pub fn should_yield(mc: &Mutation<'_>, remaining: usize) -> bool {
    remaining.is_multiple_of(DEBT_CHECK_INTERVAL)
        && mc.metrics().allocation_debt() >= f64::from(QUANTUM_DEBT)
}

/// Account for owned field payloads without depending on `IndexMap`'s private bucket layout.
pub fn record<'gc>(
    mc: &Mutation<'gc>,
    fields: crate::value::Record<'gc>,
) -> gc_arena::Gc<'gc, crate::value::Record<'gc>> {
    let bytes = fields.keys().fold(
        fields
            .capacity()
            .saturating_mul(size_of::<(String, crate::value::Value<'gc>)>()),
        |bytes, name| bytes.saturating_add(name.capacity()),
    );
    charge(mc, bytes);
    gc_arena::Gc::new(mc, fields)
}
