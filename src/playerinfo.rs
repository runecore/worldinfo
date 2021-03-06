//! PlayerInfo stuff
use anyhow::{anyhow, Context, Result};
use bitstream_io::{BigEndian, BitWrite, BitWriter};
use osrs_buffer::WriteExt;
use slab::Slab;
use std::{
    cmp,
    io::{Cursor, Write},
};

const MAX_PLAYERS: usize = 2047;
const MAX_PLAYER_MASKS: usize = 15;
const MAX_MOVEMENT_STEPS: usize = 2;

const UPDATE_GROUP_ACTIVE: i32 = 0;
const UPDATE_GROUP_INACTIVE: i32 = 1;
const REBUILD_BOUNDARY: i32 = 16;

const LOCAL_MOVEMENT_NONE: i32 = 0;
const LOCAL_MOVEMENT_WALK: i32 = 1;
const LOCAL_MOVEMENT_RUN: i32 = 2;
const LOCAL_MOVEMENT_TELEPORT: i32 = 3;

struct MovementUpdate {
    x: i32,
    y: i32,
    z: i32,
}

pub struct PlayerMasks {
    appearance_mask: Option<AppearanceMask>,
    direction_mask: Option<DirectionMask>,
}

/// The appearance mask of the player
pub struct AppearanceMask {
    pub gender: i8,
    pub skull: bool,
    pub overhead_prayer: i8,
    //pub npc: i32,
    //pub looks: PlayerLooks,
    pub head: i16,
    pub cape: i16,
    pub neck: i16,
    pub weapon: i16,
    pub body: i16,
    pub shield: i16,
    pub arms: i16,
    pub is_full_body: bool,
    pub legs: i16,
    pub hair: i16,
    pub covers_hair: bool,
    pub hands: i16,
    pub feet: i16,
    pub covers_face: bool,
    pub beard: i16,
    pub colors_hair: i8,
    pub colors_torso: i8,
    pub colors_legs: i8,
    pub colors_feet: i8,
    pub colors_skin: i8,
    pub weapon_stance_stand: i16,
    pub weapon_stance_turn: i16,
    pub weapon_stance_walk: i16,
    pub weapon_stance_turn180: i16,
    pub weapon_stance_turn90cw: i16,
    pub weapon_stance_turn90ccw: i16,
    pub weapon_stance_run: i16,
    pub username: String,
    pub combat_level: i8,
    pub skill_id_level: i16,
    pub hidden: i8,
}

/// The direction mask of the player
pub struct DirectionMask {
    pub direction: i16,
}

pub struct PlayerUpdate {
    masks: PlayerMasks,
    mask_flags: u32,
    movement_steps: Vec<(i32, i32)>,
    displaced: bool,
    movement_update: MovementUpdate,
}

/// Contains the data of the PlayerInfo entry
pub struct PlayerInfoData {
    // START RSMOD IMPL
    flags: i32,
    local: bool,
    coordinates: i32,
    reset: bool,
    // END RSMOD IMPL

    // The rest below here are custom, and might need to be revised in terms of correct structure
    local_to_global: bool,
    global_to_local: bool,
}

/// The PlayerInfo containing information about all players and their associated masks
pub struct PlayerInfo {
    // A many-to-many mapping from a player to all other players.
    // This means a player with id 0 will store data of player
    // 0, 1, 2, 3, ... 2047
    playerinfos: Slab<Slab<PlayerInfoData>>,
    // TODO: Use this field here for playermasks (or potentially just PlayerUpdates) as it will not have issues with the borrow checker
    playerupdates: Slab<PlayerUpdate>,
}

fn get_local_skip_count(
    playerinfos: &Slab<Slab<PlayerInfoData>>,
    update_group: i32,
    player_id: usize,
    offset: usize,
) -> Result<i32> {
    let mut count = 0;

    for i in offset..MAX_PLAYERS {
        // Grab the playerinfo
        let playerinfoentryother = playerinfos
            .get(player_id)
            .context("failed 1")?
            .get(i)
            .context("failed 2")?;

        // Return if the playerinfo is not in this group
        if !(playerinfoentryother.local && (update_group & 0x1) == playerinfoentryother.flags) {
            continue;
        }

        // Break if a player needs to be updated
        let is_update_required = true;
        if is_update_required {
            break;
        }

        // Increment the skip count by 1
        count += 1;
    }

    Ok(count)
}

impl Default for PlayerInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerInfo {
    /// Create a new PlayerInfo
    pub fn new() -> PlayerInfo {
        PlayerInfo {
            playerinfos: Slab::new(),
            playerupdates: Slab::new(),
        }
    }

    // TODO: Return the coordinates of all global players in this function, as to aid with the InterestInit packet
    /// Add a new player to the PlayerInfo
    pub fn add_player(&mut self, coordinates: i32) -> Result<()> {
        // Get the playerinfo id using a vacant key, check for exceeding limit
        let playerinfo_id = self.playerinfos.vacant_key();
        if playerinfo_id > MAX_PLAYERS {
            return Err(anyhow!(
                "Maximum amount of players processable by PlayerInfo reached"
            ));
        }

        // Create a new playerinfo entry
        let mut playerinfoentry = Slab::new();

        // Generate the playerinfo data for the given player
        for playerinfo in 0..MAX_PLAYERS {
            if playerinfo_id == playerinfo {
                add_playerinfodata(&mut playerinfoentry, true, coordinates)
                    .expect("failed adding update record for local player");
            }
            add_playerinfodata(&mut playerinfoentry, false, 0)
                .expect("failed adding update record for external player");
        }

        // Insert the PlayerInfoEntry
        self.playerinfos.insert(playerinfoentry);
        self.playerupdates.insert(PlayerUpdate {
            movement_steps: Vec::with_capacity(MAX_MOVEMENT_STEPS),
            displaced: false,
            movement_update: MovementUpdate { x: 0, y: 0, z: 0 },
            mask_flags: 0,
            masks: PlayerMasks {
                appearance_mask: None,
                direction_mask: None,
            },
        });

        Ok(())
    }

    /// Get the masks on the player. Useful for checking if a mask is already set
    pub fn get_player_masks(&mut self, key: usize) -> Result<&PlayerMasks> {
        let player_update = self
            .playerupdates
            .get_mut(key)
            .context("failed getting playermask vec")?;

        Ok(&player_update.masks)
    }

    pub fn add_player_appearance_mask(
        &mut self,
        player_id: usize,
        appearance_mask: AppearanceMask,
    ) -> Result<()> {
        let player_update = self
            .playerupdates
            .get_mut(player_id)
            .context("failed getting player")?;

        player_update.masks.appearance_mask = Some(appearance_mask);
        player_update.mask_flags |= APPEARANCE_MASK;

        Ok(())
    }

    pub fn add_player_direction_mask(
        &mut self,
        player_id: usize,
        direction_mask: DirectionMask,
    ) -> Result<()> {
        let player_update = self
            .playerupdates
            .get_mut(player_id)
            .context("failed getting player")?;

        player_update.masks.direction_mask = Some(direction_mask);
        player_update.mask_flags |= DIRECTION_MASK;

        Ok(())
    }

    /// TODO: Consider remove
    pub fn get_player(&mut self, key: usize) -> Option<&Slab<PlayerInfoData>> {
        self.playerinfos.get(key)
    }

    /// TODO: Consider remove
    pub fn get_player_mut(&mut self, key: usize) -> Option<&mut Slab<PlayerInfoData>> {
        self.playerinfos.get_mut(key)
    }

    /// Remove a player from the PlayerInfo
    pub fn remove_player(&mut self, key: usize) -> Result<()> {
        self.playerinfos.remove(key);
        self.playerupdates.remove(key);

        Ok(())
    }

    /// Process a player contained in the PlayerInfo, returning a buffer with data about all the updates for the specified player,
    /// to be sent
    pub fn process(&mut self, player_id: usize) -> Result<Vec<u8>> {
        // TODO: Remove this, do proper checking instead in the local_player_info and global_player_info places, simply return if the player id does not exist
        if self.playerinfos.get(player_id).is_none() {
            return Ok(Vec::new());
        }

        let mut main_buf = BitWriter::endian(Vec::new(), BigEndian);
        // Supply the mask buffer instead, as to prevent this big ass allocation
        let mut mask_buf = Cursor::new(vec![0; 60000]);

        // Write local player data (players around the player)
        self.local_player_info(player_id, &mut main_buf, &mut mask_buf, UPDATE_GROUP_ACTIVE)?;
        main_buf.byte_align()?;

        self.local_player_info(
            player_id,
            &mut main_buf,
            &mut mask_buf,
            UPDATE_GROUP_INACTIVE,
        )?;
        main_buf.byte_align()?;

        // Write global player data (players that the player cannot see)
        self.global_player_info(
            player_id,
            &mut main_buf,
            &mut mask_buf,
            UPDATE_GROUP_INACTIVE,
        )?;
        main_buf.byte_align()?;

        self.global_player_info(player_id, &mut main_buf, &mut mask_buf, UPDATE_GROUP_ACTIVE)?;
        main_buf.byte_align()?;

        // Convert the main_buf into a writer
        let mut vec = main_buf.into_writer();

        // Write the mask_buf's data
        vec.write_all(&mask_buf.get_ref()[..mask_buf.position() as usize])?;

        // Group the records
        for i in 0..MAX_PLAYERS {
            self.group(player_id, i).ok();
        }

        // Return the bit buffer including the mask buffer
        Ok(vec)
    }

    fn local_player_info(
        &mut self,
        player_id: usize,
        bit_buf: &mut BitWriter<Vec<u8>, bitstream_io::BigEndian>,
        mask_buf: &mut Cursor<Vec<u8>>,
        update_group: i32,
    ) -> Result<()> {
        let mut skip_count = 0;

        for current_player_id in 0..MAX_PLAYERS {
            // Grab the playerinfo
            let playerinfoentryother = self
                .playerinfos
                .get_mut(player_id)
                .context("failed 1")?
                .get_mut(current_player_id)
                .context("failed 2")?;

            // Test whether the playerinfo is local, and whether it is in the correct update group (active, inactive)
            if !(playerinfoentryother.local && (update_group & 0x1) == playerinfoentryother.flags) {
                continue;
            }

            // Check whether entries should be skipped
            if skip_count > 0 {
                skip_count -= 1;
                playerinfoentryother.flags |= 0x2;
                continue;
            }

            // Get the player updates
            let player_updates = self
                .playerupdates
                .get_mut(current_player_id)
                .context("testy boi")?;

            // Get whether there is mask or movement updates
            let mask_update = player_updates.mask_flags > 0;
            let movement_update =
                !player_updates.movement_steps.is_empty() || player_updates.displaced;

            // Check whether a player update is needed
            // If the player is to be removed, or it has a mask update, or it has a movement update, the first bit is set to true
            // (player update in this context)
            let player_update =
                playerinfoentryother.local_to_global || mask_update || movement_update;

            // Write the player update bool to signify whether a player needs to be updated or not
            bit_buf.write_bit(player_update)?;

            // Check if a player update is needed, else write the skip count
            if player_update {
                // Check whether the local player should be removed and turned into a global player
                if playerinfoentryother.local_to_global {
                    playerinfoentryother.reset = true;
                    remove_local_player(bit_buf, playerinfoentryother, mask_update)?;
                // Else write a movement update
                } else if movement_update {
                    write_local_movement(bit_buf, player_updates, mask_update)
                        .expect("failed writing local movement");
                // Else write to the bitbuffer that it should read masks
                } else {
                    write_mask_update_signal(bit_buf).expect("failed writing mask update signal");
                }
            } else {
                playerinfoentryother.flags |= 0x2;
                skip_count = get_local_skip_count(
                    &self.playerinfos,
                    update_group,
                    player_id,
                    current_player_id + 1,
                )?;
                write_skip_count(bit_buf, skip_count, player_update).ok();
            }

            // TODO: Move writing of masks to its own step.
            // This is only here because the borrow checker errors on "get_local_skip_count" as the PlayerInfo struct is borrowed when that function is called
            // Ideally this step should be after this whole block, so after write_skip_count.
            if mask_update {
                write_mask_update(mask_buf, player_updates)?;
            }
        }

        Ok(())
    }

    fn write_local_bit_data(&mut self) {}

    fn get_global_skip_count(
        &mut self,
        update_group: i32,
        player_id: usize,
        offset: usize,
    ) -> Result<i32> {
        let mut count = 0;

        for i in offset..MAX_PLAYERS {
            // Grab the playerinfo
            let playerinfoentryother = self
                .playerinfos
                .get_mut(player_id)
                .context("failed 1")?
                .get_mut(i)
                .context("failed 2")?;

            // Return if the playerinfo is not in this group
            if playerinfoentryother.local || (update_group & 0x1) != playerinfoentryother.flags {
                continue;
            }

            // Check here if a player needs to be added, aka they are within view distance. Simply pass over a mask

            // Increment the skip count by 1
            count += 1;
        }

        Ok(count)
    }

    fn group(&mut self, player_id: usize, index: usize) -> Result<()> {
        // Get the playerinfo
        let playerinfoentryother = self
            .playerinfos
            .get_mut(player_id)
            .context("failed getting playerinfoentry")?
            .get_mut(index)
            .context("failed playerinfoother")?;

        // Shift its flags
        playerinfoentryother.flags >>= 1;

        // Check whether the playerinfoentry should be reset
        if playerinfoentryother.reset {
            playerinfoentryother.flags = 0;
            playerinfoentryother.coordinates = 0;
            playerinfoentryother.local = false;
            playerinfoentryother.reset = false;
            playerinfoentryother.local_to_global = false;
            playerinfoentryother.global_to_local = false;
        }

        Ok(())
    }

    fn global_player_info(
        &mut self,
        player_id: usize,
        bit_buf: &mut BitWriter<Vec<u8>, bitstream_io::BigEndian>,
        mask_buf: &mut Cursor<Vec<u8>>,
        update_group: i32,
    ) -> Result<i32> {
        let mut skip_count = 0;

        for other_player_id in 0..MAX_PLAYERS {
            // Grab the playerinfo
            let playerinfoentryother = self
                .playerinfos
                .get_mut(player_id)
                .context("failed 1")?
                .get_mut(other_player_id)
                .context("failed 2")?;

            // Test whether the playerinfo is global, and whether it is in the correct update group (active, inactive)
            if playerinfoentryother.local || (update_group & 0x1) != playerinfoentryother.flags {
                continue;
            }

            // Check whether entries should be skipped
            if skip_count > 0 {
                skip_count -= 1;
                playerinfoentryother.flags |= 0x2;
                continue;
            }

            let player_update = false;
            bit_buf.write_bit(player_update)?;

            // Check whether a global player should be made local
            if playerinfoentryother.global_to_local {}

            // TODO: Make some Option type here for that a player should be added
            /*if world.players.get(i).is_some() {
                let capacity_reached = added + previously_added >= max_player_additions_per_cycle
                    || local_count >= max_local_players;

                if player_can_view_other_player(world, player_id, i) && !capacity_reached {
                    write_player_addition(bit_buf, world, player_id, i).unwrap();
                    write_new_player_masks(mask_buf, world, i);
                    *get_update_record_flags(world, player_id, i) |= 0x2;

                    // Set local to true
                    *world
                        .players
                        .get_mut(player_id)
                        .unwrap()
                        .update_record_local
                        .get_mut(i)
                        .unwrap() = true;

                    // Set the coordinate to the player's coordinate
                    *world
                        .players
                        .get_mut(player_id)
                        .unwrap()
                        .update_record_coordinates
                        .get_mut(i)
                        .unwrap() = world
                        .players
                        .get(i)
                        .unwrap()
                        .coordinates
                        .get_packed_18_bits();

                    // need it as packed 18 bits here instead of just .coords. consider making function: get_coords_as_18_bit(coords);
                    added += 1;
                }
                continue;
            }*/

            playerinfoentryother.flags |= 0x2;
            skip_count =
                self.get_global_skip_count(update_group, player_id, other_player_id + 1)?;

            write_skip_count(bit_buf, skip_count, false).ok();
        }

        Ok(0)
    }
}

fn write_skip_count(
    bit_buf: &mut BitWriter<Vec<u8>, bitstream_io::BigEndian>,
    skip_count: i32,
    player_update: bool,
) -> Result<()> {
    if skip_count == 0 {
        bit_buf.write(2, skip_count as u32)?;
    } else if skip_count < 32 {
        bit_buf.write(2, 1)?;
        bit_buf.write(5, skip_count as u32)?;
    } else if skip_count < 256 {
        bit_buf.write(2, 2)?;
        bit_buf.write(8, skip_count as u32)?;
    } else {
        if skip_count > MAX_PLAYERS as i32 {
            return Err(anyhow!("Skip count out of range error"));
        }
        bit_buf.write(2, 3)?;
        bit_buf.write(11, cmp::min(MAX_PLAYERS, skip_count as usize) as u32)?;
    }

    Ok(())
}

fn add_playerinfodata(
    playerinfo: &mut Slab<PlayerInfoData>,
    local: bool,
    coordinates: i32,
) -> Result<()> {
    playerinfo.insert(PlayerInfoData {
        flags: 0,
        local,
        coordinates,
        reset: false,
        local_to_global: false,
        global_to_local: false,
    });

    Ok(())
}

// The masks and their associated bit values
const MOVEMENT_FORCED_MASK: u32 = 0x200;
const SPOT_ANIMATION_MASK: u32 = 0x800;
const SEQUENCE_MASK: u32 = 0x80;
const APPEARANCE_MASK: u32 = 0x2;
const SHOUT_MASK: u32 = 0x20;
const LOCK_TURNTO_MASK: u32 = 0x4;
const MOVEMENT_CACHED_MASK: u32 = 0x1000;
const CHAT_MASK: u32 = 0x1;
const NAME_MODIFIERS_MASK: u32 = 0x100;
const HIT_MASK: u32 = 0x10;
const MOVEMENT_TEMPORARY_MASK: u32 = 0x400;
const DIRECTION_MASK: u32 = 0x8;

// The masks in which order they should be written out
const MASKS: [u32; 12] = [
    MOVEMENT_FORCED_MASK,
    SPOT_ANIMATION_MASK,
    SEQUENCE_MASK,
    APPEARANCE_MASK,
    SHOUT_MASK,
    LOCK_TURNTO_MASK,
    MOVEMENT_CACHED_MASK,
    CHAT_MASK,
    NAME_MODIFIERS_MASK,
    HIT_MASK,
    MOVEMENT_TEMPORARY_MASK,
    DIRECTION_MASK,
];

fn write_mask_update(mask_buf: &mut Cursor<Vec<u8>>, playerinfo: &mut PlayerUpdate) -> Result<()> {
    if playerinfo.mask_flags >= 0xFF {
        mask_buf.write_i8((playerinfo.mask_flags | 0x40) as i8)?;
        mask_buf.write_i8((playerinfo.mask_flags >> 8) as i8)?;
    } else {
        mask_buf.write_i8(playerinfo.mask_flags as i8)?;
    }

    for mask in MASKS {
        let mask_id = playerinfo.mask_flags & mask;

        match mask_id {
            APPEARANCE_MASK => write_appearance_mask(
                &playerinfo
                    .masks
                    .appearance_mask
                    .take()
                    .expect("missing appearance mask"),
                mask_buf,
            ),
            DIRECTION_MASK => write_direction_mask(
                &playerinfo
                    .masks
                    .direction_mask
                    .take()
                    .expect("missing direction mask"),
                mask_buf,
            ),
            _ => Ok(()),
        }?;
    }

    playerinfo.mask_flags = 0;

    Ok(())
}

fn remove_local_player(
    bit_buf: &mut BitWriter<Vec<u8>, bitstream_io::BigEndian>,
    playerinfo: &PlayerInfoData,
    local_player_mask_update_required: bool,
) -> Result<()> {
    let new_coordinates = 123;
    let record_coordinates = 12311;

    let coordinate_change = new_coordinates != record_coordinates;

    bit_buf.write_bit(local_player_mask_update_required)?;
    bit_buf.write(2, 0)?;
    bit_buf.write_bit(coordinate_change)?;

    if coordinate_change {
        write_coordinate_multiplier(bit_buf, record_coordinates, new_coordinates)?;
    }

    Ok(())
}

fn write_coordinate_multiplier(
    bit_buf: &mut BitWriter<Vec<u8>, bitstream_io::BigEndian>,
    old_multiplier: i32,
    new_multiplier: i32,
) -> Result<()> {
    let current_multiplier_y = new_multiplier & 0xFF;
    let current_multiplier_x = (new_multiplier >> 8) & 0xFF;
    let current_level = (new_multiplier >> 8) & 0x3;

    let last_multiplier_y = old_multiplier & 0xFF;
    let last_multiplier_x = (old_multiplier >> 8) & 0xFF;
    let last_level = (old_multiplier >> 8) & 0x3;

    let diff_x = current_multiplier_x - last_multiplier_x;
    let diff_y = current_multiplier_y - last_multiplier_y;
    let diff_level = current_level - last_level;

    let level_change = diff_level != 0;
    let small_change = diff_x.abs() <= 1 && diff_y.abs() <= 1;

    if level_change {
        bit_buf.write(2, 1)?;
        bit_buf.write(2, diff_level as u32)?;
    } else if small_change {
        let direction;

        if diff_x == -1 && diff_y == -1 {
            direction = 0;
        } else if diff_x == 1 && diff_y == -1 {
            direction = 2;
        } else if diff_x == -1 && diff_y == 1 {
            direction = 5;
        } else if diff_x == 1 && diff_y == 1 {
            direction = 7;
        } else if diff_y == -1 {
            direction = 1;
        } else if diff_x == -1 {
            direction = 3;
        } else if diff_x == 1 {
            direction = 4;
        } else {
            direction = 6;
        }

        bit_buf.write(2, 2)?;
        bit_buf.write(2, diff_level as u32)?;
        bit_buf.write(3, direction)?;
    } else {
        bit_buf.write(2, 3)?;
        bit_buf.write(2, diff_level as u32)?;
        bit_buf.write(8, diff_x as u32 & 0xFF)?;
        bit_buf.write(8, diff_y as u32 & 0xFF)?;
    }

    Ok(())
}

fn write_local_movement(
    bit_buf: &mut BitWriter<Vec<u8>, bitstream_io::BigEndian>,
    playerinfoentry: &mut PlayerUpdate,
    mask_update: bool,
) -> Result<()> {
    let direction_diff_x = [-1, 0, 1, -1, 1, -1, 0, 1];
    let direction_diff_y = [-1, -1, -1, 0, 0, 1, 1, 1];

    let movement_update = &playerinfoentry.movement_update;

    let large_change =
        movement_update.x.abs() >= REBUILD_BOUNDARY || movement_update.y.abs() >= REBUILD_BOUNDARY;
    let teleport = large_change || false;

    bit_buf.write_bit(mask_update)?;
    if teleport {
        // SKIP TELEPORT FOR NOW
        bit_buf.write(2, LOCAL_MOVEMENT_TELEPORT)?;
        bit_buf.write_bit(large_change)?;
        bit_buf.write(2, movement_update.z & 0x3)?;

        if large_change {
            bit_buf.write(14, movement_update.x & 0x3FFF)?;
            bit_buf.write(14, movement_update.y & 0x3FFF)?;
        } else {
            bit_buf.write(5, movement_update.x & 0x1F)?;
            bit_buf.write(5, movement_update.y & 0x1F)?;
        }
    } else {
        let movement_steps = &mut playerinfoentry.movement_steps;
        let walk_step = movement_steps.get(0).context("failed getting walk step")?;
        let walk_rotation = get_direction_rotation(walk_step)?;

        let mut dx = *direction_diff_x.get(walk_rotation as usize).context("dx")?;
        let mut dy = *direction_diff_y.get(walk_rotation as usize).context("dy")?;

        let mut running = false;
        let mut direction = 0;

        if let Some(run_step) = movement_steps.get(1) {
            let run_rotation = get_direction_rotation(run_step)?;

            dx += *direction_diff_x
                .get(run_rotation as usize)
                .context("dx 2")?;
            dy += *direction_diff_y
                .get(run_rotation as usize)
                .context("dy 2")?;

            if let Some(run_dir) = run_dir(dx, dy) {
                direction = run_dir;
                running = true;
            }
        }

        if !running {
            if let Some(walk_dir) = walk_dir(dx, dy) {
                direction = walk_dir;
            }
        }

        if running {
            bit_buf.write(2, LOCAL_MOVEMENT_RUN)?;
            bit_buf.write(4, direction)?;
        } else {
            bit_buf.write(2, LOCAL_MOVEMENT_WALK)?;
            bit_buf.write(3, direction)?;
        }

        movement_steps.clear();
    }

    Ok(())
}

fn write_mask_update_signal(
    bit_buf: &mut BitWriter<Vec<u8>, bitstream_io::BigEndian>,
) -> Result<()> {
    bit_buf.write_bit(true)?;
    bit_buf.write(2, LOCAL_MOVEMENT_NONE)?;

    Ok(())
}

fn write_direction_mask(
    direction_mask: &DirectionMask,
    mask_buf: &mut Cursor<Vec<u8>>,
) -> Result<()> {
    mask_buf.write_i16_add(direction_mask.direction)?;

    Ok(())
}

fn write_appearance_mask(
    appearance_mask: &AppearanceMask,
    mask_buf: &mut Cursor<Vec<u8>>,
) -> Result<()> {
    let mut temp_buf = Cursor::new(Vec::new());

    temp_buf.write_i8(appearance_mask.gender)?;
    if appearance_mask.skull {
        temp_buf.write_i8(1)?;
    } else {
        temp_buf.write_i8(-1)?;
    }

    temp_buf.write_i8(appearance_mask.overhead_prayer)?;

    // Equipment here, skipped for now
    temp_buf.write_i8(0)?; // Head
    temp_buf.write_i8(0)?; // Cape
    temp_buf.write_i8(0)?; // Neck
    temp_buf.write_i8(0)?; // Weapon

    temp_buf.write_i16(256 + 18)?; // Torso
    temp_buf.write_i8(0)?; // Shield
    temp_buf.write_i16(256 + appearance_mask.arms)?; // Arms
    temp_buf.write_i16(256 + appearance_mask.legs)?; // Legs
    temp_buf.write_i16(256 + appearance_mask.hair)?; // Hair
    temp_buf.write_i16(256 + appearance_mask.hands)?; // Hands
    temp_buf.write_i16(256 + appearance_mask.feet)?; // Feet

    if appearance_mask.gender == 0 {
        temp_buf.write_i16(256 + appearance_mask.beard)?; // Beard
    } else {
        temp_buf.write_i16(0)?;
    }

    temp_buf.write_i8(appearance_mask.colors_hair)?;
    temp_buf.write_i8(appearance_mask.colors_torso)?;
    temp_buf.write_i8(appearance_mask.colors_legs)?;
    temp_buf.write_i8(appearance_mask.colors_feet)?;
    temp_buf.write_i8(appearance_mask.colors_skin)?;

    temp_buf.write_i16(appearance_mask.weapon_stance_stand)?;
    temp_buf.write_i16(appearance_mask.weapon_stance_turn)?;
    temp_buf.write_i16(appearance_mask.weapon_stance_walk)?;
    temp_buf.write_i16(appearance_mask.weapon_stance_turn180)?;
    temp_buf.write_i16(appearance_mask.weapon_stance_turn90cw)?;
    temp_buf.write_i16(appearance_mask.weapon_stance_turn90ccw)?;
    temp_buf.write_i16(appearance_mask.weapon_stance_run)?;

    temp_buf.write_string_cp1252(&appearance_mask.username)?;
    temp_buf.write_i8(appearance_mask.combat_level)?;
    temp_buf.write_i16(appearance_mask.skill_id_level)?;
    temp_buf.write_i8(appearance_mask.hidden)?;

    mask_buf.write_i8(temp_buf.position() as i8)?;

    mask_buf.write_bytes_reversed_add(temp_buf.get_ref())?;

    Ok(())
}

fn get_direction_rotation(some_movement: &(i32, i32)) -> Result<i32> {
    match some_movement {
        (-1, -1) => Ok(0),
        (0, -1) => Ok(1),
        (1, -1) => Ok(2),
        (-1, 0) => Ok(3),
        (1, 0) => Ok(4),
        (-1, 1) => Ok(5),
        (0, 1) => Ok(6),
        (1, 1) => Ok(7),
        _ => Err(anyhow!("Failed getting direction rotation")),
    }
}

fn run_dir(dx: i32, dy: i32) -> Option<i32> {
    match (dx, dy) {
        (-2, -2) => Some(0),
        (-1, -2) => Some(1),
        (0, -2) => Some(2),
        (1, -2) => Some(3),
        (2, -2) => Some(4),
        (-2, -1) => Some(5),
        (2, -1) => Some(6),
        (-2, 0) => Some(7),
        (2, 0) => Some(8),
        (-2, 1) => Some(9),
        (2, 1) => Some(10),
        (-2, 2) => Some(11),
        (-1, 2) => Some(12),
        (0, 2) => Some(13),
        (1, 2) => Some(14),
        (2, 2) => Some(15),
        _ => None,
    }
}

fn walk_dir(dx: i32, dy: i32) -> Option<i32> {
    match (dx, dy) {
        (-1, -1) => Some(0),
        (0, -1) => Some(1),
        (1, -1) => Some(2),
        (-1, 0) => Some(3),
        (1, 0) => Some(4),
        (-1, 1) => Some(5),
        (0, 1) => Some(6),
        (1, 1) => Some(7),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_player_test() -> Result<()> {
        let mut playerinfo = PlayerInfo::new();
        playerinfo.add_player(123)?;

        assert_eq!(playerinfo.playerinfos.len(), 1);

        Ok(())
    }

    #[test]
    fn playerinfo_test() -> Result<()> {
        let mut playerinfo = PlayerInfo::new();
        playerinfo.add_player(131313)?;

        let playerinfodata = playerinfo.playerupdates.get_mut(0).context("yes")?;

        playerinfo.add_player_appearance_mask(
            0,
            AppearanceMask {
                gender: 0,
                skull: false,
                overhead_prayer: -1,
                head: 0,
                cape: 0,
                neck: 0,
                weapon: 0,
                body: 0,
                shield: 0,
                is_full_body: false,
                legs: 36,
                covers_hair: false,
                hands: 33,
                feet: 42,
                covers_face: false,
                colors_hair: 0,
                colors_torso: 0,
                colors_legs: 0,
                colors_feet: 0,
                colors_skin: 0,
                weapon_stance_stand: 808,
                weapon_stance_turn: 823,
                weapon_stance_walk: 819,
                weapon_stance_turn180: 820,
                weapon_stance_turn90cw: 821,
                weapon_stance_turn90ccw: 822,
                weapon_stance_run: 824,
                username: "Sage".to_string(),
                combat_level: 126,
                skill_id_level: 0,
                hidden: 0,
                arms: 26,
                hair: 0,
                beard: 10,
            },
        )?;

        playerinfo.add_player_direction_mask(0, DirectionMask { direction: 1536 })?;

        let vec = playerinfo.process(0)?;

        assert_eq!(
            vec,
            vec![
                192, 127, 244, 10, 50, 128, 128, 128, 254, 128, 229, 231, 225, 211, 184, 131, 182,
                131, 181, 131, 180, 131, 179, 131, 183, 131, 168, 131, 128, 128, 128, 128, 128,
                138, 129, 170, 129, 161, 129, 128, 129, 164, 129, 154, 129, 128, 146, 129, 128,
                128, 128, 128, 127, 127, 128, 6, 128
            ]
        );

        let vec = playerinfo.process(0)?;

        Ok(())
    }
}
