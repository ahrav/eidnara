//! Fixed payload-pool geometry: block classes, block ids, and mapping layout are all derived
//! by both peers from one validated `PoolGeometry`.
//!
//! Every block holds one complete frame, header included, at a fixed offset. Nothing a peer
//! writes moves a block: the receiver computes offsets from its own validated geometry and
//! checks every peer-supplied id and length against it before forming a raw view.

use std::fmt;
use std::mem::size_of;

use crate::descriptor::WIRE_V3_HEADER_BYTES;

/// Largest wire body either peer will publish or admit.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Granularity every block size and every region offset respects. The mapping is sized in
/// pages so no block shares a page with control metadata.
pub const BLOCK_ALIGN: usize = 4096;

/// Ordinary classes per direction; ascending block size.
pub const ORDINARY_CLASSES: usize = 5;

/// Upper bound on blocks per direction. Producer ledgers, receiver records, and completion
/// cells are allocated per block before a peer grant is fully trusted, so an unbounded count
/// would be an allocation attack.
pub const MAX_BLOCKS: usize = 4096;

/// Upper bound on descriptor slots per direction, ordinary plus reserved.
pub const MAX_DESCRIPTORS: usize = 4096;

/// Which inventory a reservation draws from. Ordinary traffic never spills into a reserve,
/// and a reserve never serves ordinary data, so exhausting one cannot starve the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Inventory {
    /// Application data: the smallest ordinary class that fits the bound.
    Ordinary,
    /// Pure-header and small controls: the reserved control class.
    Control,
    /// Terminals and rejections: the reserved terminal class.
    Terminal,
}

/// One block class: how many blocks and how many bytes each block holds, header included.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ClassSpec {
    /// Full-frame capacity of one block: header plus the largest body it can hold.
    pub block_bytes: u64,
    /// Blocks in this class.
    pub count: u32,
}

impl ClassSpec {
    /// `block_bytes` and `count`, unchecked; `PoolGeometry::new` validates.
    pub const fn new(block_bytes: u64, count: u32) -> Self {
        Self { block_bytes, count }
    }

    /// Largest body one block of this class can carry.
    pub const fn body_capacity(self) -> u64 {
        self.block_bytes.saturating_sub(WIRE_V3_HEADER_BYTES as u64)
    }
}

impl fmt::Debug for ClassSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "ClassSpec({} x {} bytes)",
            self.count, self.block_bytes
        )
    }
}

/// Where one block lives among the inventories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockClass {
    /// Ordinary class `index` in ascending size order.
    Ordinary(u8),
    /// Reserved control class.
    Control,
    /// Reserved terminal class.
    Terminal,
}

impl BlockClass {
    /// Inventory this class belongs to.
    pub const fn inventory(self) -> Inventory {
        match self {
            Self::Ordinary(_) => Inventory::Ordinary,
            Self::Control => Inventory::Control,
            Self::Terminal => Inventory::Terminal,
        }
    }

    /// Dense index over the seven classes, ordinary first.
    pub const fn index(self) -> usize {
        match self {
            Self::Ordinary(index) => index as usize,
            Self::Control => ORDINARY_CLASSES,
            Self::Terminal => ORDINARY_CLASSES + 1,
        }
    }

    /// Inverse of `index`.
    pub const fn from_index(index: usize) -> Option<Self> {
        if index < ORDINARY_CLASSES {
            Some(Self::Ordinary(index as u8))
        } else if index == ORDINARY_CLASSES {
            Some(Self::Control)
        } else if index == ORDINARY_CLASSES + 1 {
            Some(Self::Terminal)
        } else {
            None
        }
    }
}

/// Number of classes across all inventories.
pub const CLASS_COUNT: usize = ORDINARY_CLASSES + 2;

/// Geometry of one direction: descriptor depths and the seven block classes. Validated once by
/// `new`; both peers must hold identical values or attachment fails.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PoolGeometry {
    ordinary_descriptors: u32,
    reserved_descriptors: u32,
    classes: [ClassSpec; CLASS_COUNT],
}

/// Fixed position of one block in the arena, derived from validated geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockPlacement {
    /// Which class the block belongs to.
    pub class: BlockClass,
    /// Offset of the block's first byte (its header) from the arena start.
    pub offset: u64,
    /// Full-frame capacity of the block.
    pub block_bytes: u64,
}

impl BlockPlacement {
    /// Largest body the block can carry.
    pub const fn body_capacity(&self) -> u64 {
        self.block_bytes.saturating_sub(WIRE_V3_HEADER_BYTES as u64)
    }
}

impl PoolGeometry {
    /// Validates the geometry. Ordinary classes must ascend strictly, every block size must
    /// be a nonzero multiple of `BLOCK_ALIGN`, every class must hold at least one block, and
    /// the block and descriptor totals must stay within `MAX_BLOCKS` and `MAX_DESCRIPTORS`.
    /// Whether the largest class holds one maximum frame is a profile rule
    /// (`holds_maximum_frame`), so tests can build small pools while every shipped profile
    /// keeps a legal frame placeable.
    pub fn new(
        ordinary_descriptors: u32,
        reserved_descriptors: u32,
        ordinary: [ClassSpec; ORDINARY_CLASSES],
        control: ClassSpec,
        terminal: ClassSpec,
    ) -> Result<Self, GeometryError> {
        if ordinary_descriptors == 0 {
            return Err(GeometryError::ZeroDescriptors);
        }
        let descriptors = u64::from(ordinary_descriptors) + u64::from(reserved_descriptors);
        if descriptors > MAX_DESCRIPTORS as u64 {
            return Err(GeometryError::TooManyDescriptors);
        }
        let mut classes = [ClassSpec::new(0, 0); CLASS_COUNT];
        let mut previous = 0u64;
        for (index, class) in ordinary.into_iter().enumerate() {
            Self::check_class(class)?;
            if class.block_bytes <= previous {
                return Err(GeometryError::ClassesNotAscending);
            }
            previous = class.block_bytes;
            classes[index] = class;
        }
        Self::check_class(control)?;
        Self::check_class(terminal)?;
        classes[ORDINARY_CLASSES] = control;
        classes[ORDINARY_CLASSES + 1] = terminal;
        let geometry = Self {
            ordinary_descriptors,
            reserved_descriptors,
            classes,
        };
        let blocks = geometry
            .classes
            .iter()
            .try_fold(0u64, |sum, class| sum.checked_add(u64::from(class.count)))
            .ok_or(GeometryError::ArithmeticOverflow)?;
        if blocks > MAX_BLOCKS as u64 {
            return Err(GeometryError::TooManyBlocks);
        }
        geometry.arena_bytes()?;
        Ok(geometry)
    }

    fn check_class(class: ClassSpec) -> Result<(), GeometryError> {
        if class.count == 0 {
            return Err(GeometryError::EmptyClass);
        }
        if class.block_bytes == 0
            || !class.block_bytes.is_multiple_of(BLOCK_ALIGN as u64)
            || class.block_bytes <= WIRE_V3_HEADER_BYTES as u64
        {
            return Err(GeometryError::InvalidBlockBytes);
        }
        Ok(())
    }

    /// Descriptor slots ordinary traffic may occupy at once.
    pub const fn ordinary_descriptors(&self) -> u32 {
        self.ordinary_descriptors
    }

    /// Descriptor slots held back for reserved control and terminal traffic.
    pub const fn reserved_descriptors(&self) -> u32 {
        self.reserved_descriptors
    }

    /// Total descriptor slots: ordinary plus reserved.
    pub const fn descriptor_depth(&self) -> u32 {
        self.ordinary_descriptors + self.reserved_descriptors
    }

    /// The seven classes, ordinary first in ascending size.
    pub const fn classes(&self) -> &[ClassSpec; CLASS_COUNT] {
        &self.classes
    }

    /// The class at dense index `class`.
    pub fn class(&self, class: BlockClass) -> ClassSpec {
        self.classes[class.index()]
    }

    /// Total blocks across every class; also the number of completion cells and return
    /// records.
    pub fn block_count(&self) -> u32 {
        self.classes.iter().map(|class| class.count).sum()
    }

    /// Bytes the block arena occupies: every class region back to back.
    pub fn arena_bytes(&self) -> Result<u64, GeometryError> {
        self.classes.iter().try_fold(0u64, |sum, class| {
            class
                .block_bytes
                .checked_mul(u64::from(class.count))
                .and_then(|region| sum.checked_add(region))
                .ok_or(GeometryError::ArithmeticOverflow)
        })
    }

    /// First block id of `class`; ids are dense across classes in `classes()` order.
    pub fn first_block(&self, class: BlockClass) -> u32 {
        self.classes[..class.index()]
            .iter()
            .map(|class| class.count)
            .sum()
    }

    /// Smallest ordinary class whose body capacity covers `body_bytes`, or `None` when no
    /// class can hold the body. Ordinary traffic never spills into a reserve.
    pub fn ordinary_class_for(&self, body_bytes: u64) -> Option<BlockClass> {
        (0..ORDINARY_CLASSES)
            .find(|index| self.classes[*index].body_capacity() >= body_bytes)
            .map(|index| BlockClass::Ordinary(index as u8))
    }

    /// The class a reservation from `inventory` draws for `body_bytes`, or `None` when the
    /// inventory cannot hold the body.
    pub fn class_for(&self, inventory: Inventory, body_bytes: u64) -> Option<BlockClass> {
        match inventory {
            Inventory::Ordinary => self.ordinary_class_for(body_bytes),
            Inventory::Control => (self.class(BlockClass::Control).body_capacity() >= body_bytes)
                .then_some(BlockClass::Control),
            Inventory::Terminal => (self.class(BlockClass::Terminal).body_capacity() >= body_bytes)
                .then_some(BlockClass::Terminal),
        }
    }

    /// Where block `id` lives, or `None` when `id` is past `block_count`. The offset and
    /// capacity come from this geometry alone; a peer cannot move a block.
    pub fn placement(&self, id: u32) -> Option<BlockPlacement> {
        let mut first = 0u32;
        let mut offset = 0u64;
        for (index, class) in self.classes.iter().enumerate() {
            if id < first.checked_add(class.count)? {
                let within = u64::from(id - first);
                return Some(BlockPlacement {
                    class: BlockClass::from_index(index)?,
                    offset: offset.checked_add(within.checked_mul(class.block_bytes)?)?,
                    block_bytes: class.block_bytes,
                });
            }
            first = first.checked_add(class.count)?;
            offset = offset.checked_add(class.block_bytes.checked_mul(u64::from(class.count))?)?;
        }
        None
    }

    /// Whether `id` names a block.
    pub fn contains(&self, id: u32) -> bool {
        id < self.block_count()
    }

    /// Whether the largest ordinary class can carry one `MAX_FRAME_BYTES` body, so exhaustion
    /// is backpressure rather than a smaller interoperability limit.
    pub fn holds_maximum_frame(&self) -> bool {
        self.classes[ORDINARY_CLASSES - 1].body_capacity() >= MAX_FRAME_BYTES as u64
    }

    /// Geometry of the sole production profile: 64x4 KiB, 16x64 KiB, 8x1 MiB, 2x8 MiB, and
    /// 1x(64 MiB + 4 KiB) ordinary blocks; 32x4 KiB control blocks; 64x32 KiB terminal blocks;
    /// 32 ordinary and 16 reserved descriptor slots.
    pub fn host_payload_pool() -> Self {
        const KIB: u64 = 1024;
        const MIB: u64 = 1024 * KIB;
        Self::new(
            32,
            16,
            [
                ClassSpec::new(4 * KIB, 64),
                ClassSpec::new(64 * KIB, 16),
                ClassSpec::new(MIB, 8),
                ClassSpec::new(8 * MIB, 2),
                ClassSpec::new(64 * MIB + 4 * KIB, 1),
            ],
            ClassSpec::new(4 * KIB, 32),
            ClassSpec::new(32 * KIB, 64),
        )
        .expect("the production geometry is valid")
    }
}

impl fmt::Debug for PoolGeometry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PoolGeometry")
            .field("ordinary_descriptors", &self.ordinary_descriptors)
            .field("reserved_descriptors", &self.reserved_descriptors)
            .field("classes", &self.classes)
            .finish()
    }
}

/// Why `PoolGeometry::new` refused a geometry.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GeometryError {
    /// No ordinary descriptor slot.
    #[error("pool geometry has no ordinary descriptor")]
    ZeroDescriptors,
    /// Ordinary plus reserved descriptors exceed `MAX_DESCRIPTORS`.
    #[error("pool geometry exceeds the descriptor bound")]
    TooManyDescriptors,
    /// Ordinary classes do not strictly ascend.
    #[error("pool classes must ascend strictly")]
    ClassesNotAscending,
    /// A class holds zero blocks.
    #[error("pool class holds no block")]
    EmptyClass,
    /// A block size is zero, not page aligned, or no larger than the header.
    #[error("pool block size is invalid")]
    InvalidBlockBytes,
    /// Total blocks exceed `MAX_BLOCKS`.
    #[error("pool geometry exceeds the block bound")]
    TooManyBlocks,
    /// A size or count product overflowed.
    #[error("pool geometry arithmetic overflow")]
    ArithmeticOverflow,
}

impl fmt::Debug for GeometryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Byte offsets of every region in one direction's mapping. Both peers compute this from the
/// same geometry, so a grant whose `total_bytes` disagrees with the computed total is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappingLayout {
    /// Producer control page.
    pub producer: usize,
    /// Consumer control page.
    pub consumer: usize,
    /// Data-ready wake epoch.
    pub data_wake: usize,
    /// Capacity-ready wake epoch.
    pub capacity_wake: usize,
    /// Lifecycle page.
    pub lifecycle: usize,
    /// First descriptor slot.
    pub descriptors: usize,
    /// Descriptor slots.
    pub descriptor_depth: usize,
    /// First completion cell.
    pub completions: usize,
    /// Completion cells, one per block.
    pub block_count: usize,
    /// First return-summary word. Each bit names a block whose completion cell was written
    /// after the producer last cleared the word; reclamation visits only those blocks.
    pub returns: usize,
    /// Return-summary words: `block_count` bits rounded up to whole `u64`s.
    pub return_words: usize,
    /// First arena byte.
    pub arena: usize,
    /// Arena bytes.
    pub arena_bytes: usize,
    /// Whole mapping.
    pub total: usize,
}

/// Bytes of one control page; every control structure is padded to this.
pub const CACHELINE: usize = 128;
/// Bytes of one descriptor slot.
pub const DESCRIPTOR_SLOT_BYTES: usize = 64;
/// Bytes of the lifecycle page, which carries the full geometry.
pub const LIFECYCLE_PAGE_BYTES: usize = 256;

impl MappingLayout {
    /// Computes offsets for `geometry` with `page_size` alignment for the arena and total.
    pub fn new(geometry: &PoolGeometry, page_size: usize) -> Result<Self, GeometryError> {
        if page_size == 0 || !page_size.is_power_of_two() || !page_size.is_multiple_of(BLOCK_ALIGN)
        {
            return Err(GeometryError::InvalidBlockBytes);
        }
        let overflow = GeometryError::ArithmeticOverflow;
        let producer = 0usize;
        let consumer = producer.checked_add(CACHELINE).ok_or(overflow)?;
        let data_wake = consumer.checked_add(CACHELINE).ok_or(overflow)?;
        let capacity_wake = data_wake.checked_add(CACHELINE).ok_or(overflow)?;
        let lifecycle = capacity_wake.checked_add(CACHELINE).ok_or(overflow)?;
        let descriptors = lifecycle
            .checked_add(LIFECYCLE_PAGE_BYTES)
            .ok_or(overflow)?;
        let descriptor_depth = geometry.descriptor_depth() as usize;
        let descriptor_bytes = descriptor_depth
            .checked_mul(DESCRIPTOR_SLOT_BYTES)
            .ok_or(overflow)?;
        let completions = align_up(
            descriptors.checked_add(descriptor_bytes).ok_or(overflow)?,
            CACHELINE,
        )?;
        let block_count = geometry.block_count() as usize;
        let completion_bytes = block_count.checked_mul(size_of::<u64>()).ok_or(overflow)?;
        let returns = align_up(
            completions.checked_add(completion_bytes).ok_or(overflow)?,
            CACHELINE,
        )?;
        let return_words = block_count.div_ceil(RETURN_WORD_BITS);
        let return_bytes = return_words.checked_mul(size_of::<u64>()).ok_or(overflow)?;
        let arena = align_up(
            returns.checked_add(return_bytes).ok_or(overflow)?,
            page_size,
        )?;
        let arena_bytes = usize::try_from(geometry.arena_bytes()?).map_err(|_| overflow)?;
        let total = align_up(arena.checked_add(arena_bytes).ok_or(overflow)?, page_size)?;
        Ok(Self {
            producer,
            consumer,
            data_wake,
            capacity_wake,
            lifecycle,
            descriptors,
            descriptor_depth,
            completions,
            block_count,
            returns,
            return_words,
            arena,
            arena_bytes,
            total,
        })
    }

    /// Offset of descriptor slot `index`, refused at or past the depth.
    pub fn descriptor_offset(&self, index: usize) -> Option<usize> {
        if index >= self.descriptor_depth {
            return None;
        }
        self.descriptors
            .checked_add(index.checked_mul(DESCRIPTOR_SLOT_BYTES)?)
    }

    /// Offset of completion cell `block`, refused at or past the block count.
    pub fn completion_offset(&self, block: usize) -> Option<usize> {
        if block >= self.block_count {
            return None;
        }
        self.completions
            .checked_add(block.checked_mul(size_of::<u64>())?)
    }

    /// Offset of return-summary word `word`, refused at or past `return_words`.
    pub fn return_offset(&self, word: usize) -> Option<usize> {
        if word >= self.return_words {
            return None;
        }
        self.returns
            .checked_add(word.checked_mul(size_of::<u64>())?)
    }
}

/// Blocks one return-summary word covers.
pub const RETURN_WORD_BITS: usize = u64::BITS as usize;

/// Rounds `value` up to a multiple of `alignment`, which must be a power of two.
pub fn align_up(value: usize, alignment: usize) -> Result<usize, GeometryError> {
    let mask = alignment - 1;
    value
        .checked_add(mask)
        .map(|sum| sum & !mask)
        .ok_or(GeometryError::ArithmeticOverflow)
}

/// Bytes the private producer ledger and receiver records occupy for `geometry`, charged
/// beside the mapping so admission counts every allocation activation performs.
pub fn ledger_bytes(geometry: &PoolGeometry) -> u64 {
    // Producer: per block one state word and one generation, one free-list entry, one
    // outstanding-list entry, and one reclaimed-list entry. Receiver: per block one live
    // generation and one last-seen generation.
    let blocks = u64::from(geometry.block_count());
    let per_block = (4 * size_of::<u64>() + 3 * size_of::<u32>()) as u64;
    blocks.saturating_mul(per_block)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> PoolGeometry {
        PoolGeometry::new(
            2,
            1,
            [
                ClassSpec::new(4096, 2),
                ClassSpec::new(8192, 1),
                ClassSpec::new(16384, 1),
                ClassSpec::new(32768, 1),
                ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
            ],
            ClassSpec::new(4096, 1),
            ClassSpec::new(32768, 1),
        )
        .unwrap()
    }

    #[test]
    fn production_geometry_matches_the_specified_inventory() {
        let geometry = PoolGeometry::host_payload_pool();
        assert_eq!(geometry.ordinary_descriptors(), 32);
        assert_eq!(geometry.reserved_descriptors(), 16);
        assert_eq!(geometry.descriptor_depth(), 48);
        assert_eq!(geometry.block_count(), 187);
        let ordinary: u32 = geometry.classes()[..ORDINARY_CLASSES]
            .iter()
            .map(|class| class.count)
            .sum();
        assert_eq!(ordinary, 91);
        // 89.254 MiB ordinary plus 2.125 MiB reserved.
        assert_eq!(geometry.arena_bytes().unwrap(), 95_817_728);
        let largest = geometry.class(BlockClass::Ordinary(4));
        assert_eq!(largest.body_capacity(), 64 * 1024 * 1024 + 4096 - 21);
        assert!(largest.body_capacity() >= MAX_FRAME_BYTES as u64);
    }

    #[test]
    fn placement_is_dense_and_offsets_never_overlap() {
        let geometry = tiny();
        let mut end = 0u64;
        for id in 0..geometry.block_count() {
            let placement = geometry.placement(id).unwrap();
            assert_eq!(
                placement.offset, end,
                "block {id} starts where the last ended"
            );
            end += placement.block_bytes;
        }
        assert_eq!(end, geometry.arena_bytes().unwrap());
        assert_eq!(geometry.placement(geometry.block_count()), None);
        assert_eq!(geometry.first_block(BlockClass::Control), 6);
        assert_eq!(geometry.first_block(BlockClass::Terminal), 7);
    }

    #[test]
    fn class_selection_takes_the_smallest_fit_without_spill() {
        let geometry = tiny();
        assert_eq!(
            geometry.class_for(Inventory::Ordinary, 0),
            Some(BlockClass::Ordinary(0))
        );
        assert_eq!(
            geometry.class_for(Inventory::Ordinary, 4096 - 21),
            Some(BlockClass::Ordinary(0))
        );
        assert_eq!(
            geometry.class_for(Inventory::Ordinary, 4096 - 20),
            Some(BlockClass::Ordinary(1))
        );
        assert_eq!(
            geometry.class_for(Inventory::Ordinary, MAX_FRAME_BYTES as u64),
            Some(BlockClass::Ordinary(4))
        );
        assert_eq!(
            geometry.class_for(Inventory::Ordinary, MAX_FRAME_BYTES as u64 + 4096),
            None
        );
        assert_eq!(
            geometry.class_for(Inventory::Control, 4096 - 21),
            Some(BlockClass::Control)
        );
        assert_eq!(geometry.class_for(Inventory::Control, 4096 - 20), None);
        assert_eq!(
            geometry.class_for(Inventory::Terminal, 32768 - 21),
            Some(BlockClass::Terminal)
        );
        assert_eq!(geometry.class_for(Inventory::Terminal, 32768 - 20), None);
    }

    #[test]
    fn geometry_rejects_every_invalid_shape() {
        let ok = tiny();
        let classes = *ok.classes();
        let ordinary: [ClassSpec; ORDINARY_CLASSES] =
            classes[..ORDINARY_CLASSES].try_into().unwrap();
        let control = classes[ORDINARY_CLASSES];
        let terminal = classes[ORDINARY_CLASSES + 1];
        assert_eq!(
            PoolGeometry::new(0, 1, ordinary, control, terminal).err(),
            Some(GeometryError::ZeroDescriptors)
        );
        assert_eq!(
            PoolGeometry::new(4000, 100, ordinary, control, terminal).err(),
            Some(GeometryError::TooManyDescriptors)
        );
        let mut unordered = ordinary;
        unordered.swap(0, 1);
        assert_eq!(
            PoolGeometry::new(2, 1, unordered, control, terminal).err(),
            Some(GeometryError::ClassesNotAscending)
        );
        let mut empty = ordinary;
        empty[0].count = 0;
        assert_eq!(
            PoolGeometry::new(2, 1, empty, control, terminal).err(),
            Some(GeometryError::EmptyClass)
        );
        let mut unaligned = ordinary;
        unaligned[0].block_bytes = 4000;
        assert_eq!(
            PoolGeometry::new(2, 1, unaligned, control, terminal).err(),
            Some(GeometryError::InvalidBlockBytes)
        );
        let mut small = ordinary;
        small[4].block_bytes = 64 * 1024 * 1024;
        assert!(
            !PoolGeometry::new(2, 1, small, control, terminal)
                .unwrap()
                .holds_maximum_frame()
        );
        assert!(ok.holds_maximum_frame());
        let mut many = ordinary;
        many[0].count = 5000;
        assert_eq!(
            PoolGeometry::new(2, 1, many, control, terminal).err(),
            Some(GeometryError::TooManyBlocks)
        );
    }

    #[test]
    fn layout_places_every_region_in_order_and_pads_to_pages() {
        let geometry = tiny();
        let layout = MappingLayout::new(&geometry, 4096).unwrap();
        assert_eq!(layout.producer, 0);
        assert_eq!(layout.consumer, CACHELINE);
        assert_eq!(layout.lifecycle, 4 * CACHELINE);
        assert_eq!(layout.descriptors, 4 * CACHELINE + LIFECYCLE_PAGE_BYTES);
        assert!(layout.completions.is_multiple_of(CACHELINE));
        assert!(layout.returns.is_multiple_of(CACHELINE));
        assert!(layout.returns >= layout.completions + layout.block_count * 8);
        assert_eq!(layout.return_words, 1);
        assert!(layout.arena.is_multiple_of(4096));
        assert!(layout.arena >= layout.returns + layout.return_words * 8);
        assert_eq!(layout.arena_bytes as u64, geometry.arena_bytes().unwrap());
        assert!(layout.total.is_multiple_of(4096));
        assert!(layout.total >= layout.arena + layout.arena_bytes);
        assert_eq!(layout.descriptor_offset(layout.descriptor_depth), None);
        assert_eq!(layout.completion_offset(layout.block_count), None);
        assert_eq!(layout.return_offset(layout.return_words), None);
        assert_eq!(layout.return_offset(0), Some(layout.returns));
        assert_eq!(
            layout.descriptor_offset(1),
            Some(layout.descriptors + DESCRIPTOR_SLOT_BYTES)
        );
        assert_eq!(layout.completion_offset(2), Some(layout.completions + 16));
        assert!(MappingLayout::new(&geometry, 3000).is_err());

        let production = MappingLayout::new(&PoolGeometry::host_payload_pool(), 4096).unwrap();
        assert_eq!(production.return_words, 3);
        assert_eq!(production.returns, 5376);
        assert_eq!(production.arena, 8192);
        assert_eq!(production.total, 95_825_920);
    }
}
