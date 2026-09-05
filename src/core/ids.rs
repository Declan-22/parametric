// Generational IDs: stable handles into arena storage. A stale ID (pointing
// at a slot whose generation moved on) resolves to None instead of wrong data.

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name {
            pub idx: u32,
            pub generation: u32,
        }

        impl $name {
            pub const NONE: Self = Self { idx: u32::MAX, generation: 0 };

            pub fn is_some(self) -> bool {
                self.idx != u32::MAX
            }
        }
    };
}

define_id!(PointId);
define_id!(SegmentId);
define_id!(FillId);
define_id!(PathId);
