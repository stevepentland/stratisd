// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Code to handle the backing store of a pool.

use std::path::Path;

use devicemapper::{Sectors, IEC};

use stratis::{ErrorEnum, StratisError, StratisResult};

use super::super::super::types::{BlockDevTier, DevUuid, PoolUuid};

use super::blockdev::StratBlockDev;
use super::blockdevmgr::{coalesce_blkdevsegs, BlkDevSegment, BlockDevMgr, Segment};

/// Handles the cache devices.
#[derive(Debug)]
pub struct CacheTier {
    /// Manages the individual block devices
    pub block_mgr: BlockDevMgr,
    /// The list of segments granted by block_mgr and used by the cache
    /// device.
    pub cache_segments: Vec<BlkDevSegment>,
    /// The list of segments granted by block_mgr and used by the metadata
    /// device.
    pub meta_segments: Vec<BlkDevSegment>,
}

impl CacheTier {
    /// Setup a previously existing cache layer from the block_mgr and
    /// previously allocated segments.
    pub fn setup(
        block_mgr: BlockDevMgr,
        cache_segments: &[(DevUuid, Sectors, Sectors)],
        meta_segments: &[(DevUuid, Sectors, Sectors)],
    ) -> StratisResult<CacheTier> {
        if block_mgr.avail_space() != Sectors(0) {
            let err_msg = format!(
                "{} unallocated to device; probable metadata corruption",
                block_mgr.avail_space()
            );
            return Err(StratisError::Engine(ErrorEnum::Error, err_msg));
        }

        let uuid_to_devno = block_mgr.uuid_to_devno();
        let mapper = |triple: &(DevUuid, Sectors, Sectors)| -> StratisResult<BlkDevSegment> {
            let device = uuid_to_devno(triple.0).ok_or_else(|| {
                StratisError::Engine(
                    ErrorEnum::NotFound,
                    format!("missing device for UUUD {:?}", &triple.0),
                )
            })?;
            Ok(BlkDevSegment::new(
                triple.0,
                Segment::new(device, triple.1, triple.2),
            ))
        };

        let meta_segments = meta_segments
            .iter()
            .map(&mapper)
            .collect::<StratisResult<Vec<_>>>()?;

        let cache_segments = cache_segments
            .iter()
            .map(&mapper)
            .collect::<StratisResult<Vec<_>>>()?;

        Ok(CacheTier {
            block_mgr,
            meta_segments,
            cache_segments,
        })
    }

    /// Add the given paths to self. Return UUIDs of the new blockdevs
    /// corresponding to the specified paths.
    /// Adds all additional space to cache sub-device.
    /// WARNING: metadata changing event
    // FIXME: That all segments on the newly added device are added to the
    // cache sub-device and none to the meta sub-device could lead to failure.
    // Presumably, the size required for the meta sub-device varies directly
    // with the size of cache sub-device.
    pub fn add(
        &mut self,
        pool_uuid: PoolUuid,
        paths: &[&Path],
        force: bool,
    ) -> StratisResult<Vec<DevUuid>> {
        let uuids = self.block_mgr.add(pool_uuid, paths, force)?;

        let avail_space = self.block_mgr.avail_space();
        let segments = self.block_mgr
            .alloc_space(&[avail_space])
            .expect("asked for exactly the space available, must get")
            .iter()
            .flat_map(|s| s.iter())
            .cloned()
            .collect::<Vec<_>>();
        self.cache_segments = coalesce_blkdevsegs(&self.cache_segments, &segments);

        Ok(uuids)
    }

    /// Setup a new CacheTier struct from the block_mgr.
    ///
    /// WARNING: metadata changing event
    pub fn new(mut block_mgr: BlockDevMgr) -> CacheTier {
        let avail_space = block_mgr.avail_space();

        // FIXME: Come up with a better way to choose metadata device size
        let meta_space = Sectors(IEC::Mi);

        assert!(
            meta_space < avail_space,
            "every block device must be at least one GiB"
        );

        let mut segments = block_mgr
            .alloc_space(&[meta_space, avail_space - meta_space])
            .expect("asked for exactly the space available, must get");

        let cache_segments = segments.pop().expect("segments.len() == 2");
        let meta_segments = segments.pop().expect("segments.len() == 1");

        CacheTier {
            block_mgr,
            meta_segments,
            cache_segments,
        }
    }

    /// Destroy the tier. Wipe its blockdevs.
    pub fn destroy(self) -> StratisResult<()> {
        self.block_mgr.destroy_all()
    }

    /// Get all the blockdevs belonging to this tier.
    pub fn blockdevs(&self) -> Vec<(DevUuid, &StratBlockDev)> {
        self.block_mgr.blockdevs()
    }

    pub fn blockdevs_mut(&mut self) -> Vec<(DevUuid, &mut StratBlockDev)> {
        self.block_mgr.blockdevs_mut()
    }

    /// Lookup an immutable blockdev by its Stratis UUID.
    pub fn get_blockdev_by_uuid(&self, uuid: DevUuid) -> Option<(BlockDevTier, &StratBlockDev)> {
        self.block_mgr
            .get_blockdev_by_uuid(uuid)
            .and_then(|bd| Some((BlockDevTier::Cache, bd)))
    }

    /// Lookup a mutable blockdev by its Stratis UUID.
    pub fn get_mut_blockdev_by_uuid(
        &mut self,
        uuid: DevUuid,
    ) -> Option<(BlockDevTier, &mut StratBlockDev)> {
        self.block_mgr
            .get_mut_blockdev_by_uuid(uuid)
            .and_then(|bd| Some((BlockDevTier::Cache, bd)))
    }
}