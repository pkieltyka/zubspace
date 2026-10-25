use crate::continuum::{decompress_zlib_payload_bounded, sign_extend_i16};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::packet_io::{read_i16, read_u16, read_u32};
use crate::render_limits::image_allocation_for_filename;
use crate::zone_wire::{
    LVZ_FILE_RETAINED_BASE_BYTES, LVZ_FILENAME_REFERENCE_RETAINED_BYTES,
    LVZ_IMAGE_RETAINED_BASE_BYTES, LVZ_OBJECT_RETAINED_BYTES, LVZ_RENDER_REFERENCE_RETAINED_BYTES,
    LVZ_TEXTURE_RETAINED_BASE_BYTES, MAX_LVZ_FILENAME_BYTES, MAX_LVZ_IMAGES_PER_ZONE,
    MAX_LVZ_OBJECT_REFERENCES_PER_ZONE, MAX_LVZ_OBJECTS_PER_ZONE, MAX_LVZ_TEXTURES_PER_ZONE,
    MAX_ZONE_IMAGE_DECODE_WORK_BYTES, MAX_ZONE_RENDER_BYTES, MAX_ZONE_RENDER_PIXEL_BYTES,
};

#[derive(Clone, Debug)]
pub(crate) struct LvzArchive {
    // Immutable after structural parsing: a validation seal and prepared
    // pixels can never outlive changes to their exact section bytes/order.
    sections: Vec<LvzSection>,
    fully_validated: bool,
    prepared_images: Option<PreparedLvzImages>,
}

impl PartialEq for LvzArchive {
    fn eq(&self, other: &Self) -> bool {
        self.sections == other.sections && self.fully_validated == other.fully_validated
    }
}

impl Eq for LvzArchive {}

#[derive(Clone)]
struct PreparedLvzImages(Arc<Mutex<Vec<Option<crate::bm2::DecodedImage>>>>);

const PREPARED_ARCHIVE_BOOKKEEPING_BYTES: usize = 256;

impl std::fmt::Debug for PreparedLvzImages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let slots = self.0.lock().expect("prepared image lock is not poisoned");
        f.debug_struct("PreparedLvzImages")
            .field(
                "remaining",
                &slots.iter().filter(|slot| slot.is_some()).count(),
            )
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LvzSection {
    pub(crate) filename: Arc<str>,
    pub(crate) timestamp: u32,
    pub(crate) data: Arc<[u8]>,
    pub(crate) kind: LvzSectionKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LvzSectionKind {
    File,
    Objects(LvzObjectSection),
}

struct LvzSectionDescriptor<'a> {
    filename: &'a str,
    timestamp: u32,
    decompressed_size: usize,
    compressed_size: usize,
    raw: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LvzObjectSection {
    pub(crate) version: u32,
    pub(crate) objects: Vec<LvzObjectDefinition>,
    pub(crate) images: Vec<LvzImageDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LvzObjectDefinition {
    pub(crate) map_object: bool,
    pub(crate) object_id: u16,
    pub(crate) x_map: i16,
    pub(crate) y_map: i16,
    pub(crate) x_screen: i16,
    pub(crate) y_screen: i16,
    pub(crate) x_type: u8,
    pub(crate) y_type: u8,
    pub(crate) image_number: u8,
    pub(crate) layer: u8,
    pub(crate) display_time: u16,
    pub(crate) display_mode: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LvzImageDefinition {
    pub(crate) x_count: i16,
    pub(crate) y_count: i16,
    pub(crate) animation_time: i16,
    pub(crate) filename: Arc<str>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LvzRenderUsage {
    pub(crate) decoded_source_bytes: usize,
    pub(crate) metadata_bytes: usize,
    pub(crate) pixel_bytes: usize,
    /// Sum of RGBA output bytes for every renderable File payload before
    /// normalized last-wins coalescing. This bounds CPU decode work rather
    /// than retained texture state.
    pub(crate) image_decode_work_bytes: usize,
    pub(crate) object_count: usize,
    pub(crate) image_count: usize,
    pub(crate) reference_count: usize,
    pub(crate) texture_count: usize,
    pub(crate) chat_sound_count: usize,
    pub(crate) chat_sound_pcm_bytes: usize,
}

impl LvzRenderUsage {
    pub(crate) fn retained_bytes(self) -> Result<usize, String> {
        self.decoded_source_bytes
            .checked_add(self.metadata_bytes)
            .and_then(|total| total.checked_add(self.pixel_bytes))
            .and_then(|total| total.checked_add(self.chat_sound_pcm_bytes))
            .ok_or_else(|| "LVZ retained render size overflow".to_string())
    }

    pub(crate) fn checked_add(self, other: Self) -> Result<Self, String> {
        let usage = Self {
            decoded_source_bytes: self
                .decoded_source_bytes
                .checked_add(other.decoded_source_bytes)
                .ok_or_else(|| "LVZ decoded source aggregate overflow".to_string())?,
            metadata_bytes: self
                .metadata_bytes
                .checked_add(other.metadata_bytes)
                .ok_or_else(|| "LVZ metadata aggregate overflow".to_string())?,
            pixel_bytes: self
                .pixel_bytes
                .checked_add(other.pixel_bytes)
                .ok_or_else(|| "LVZ pixel aggregate overflow".to_string())?,
            image_decode_work_bytes: self
                .image_decode_work_bytes
                .checked_add(other.image_decode_work_bytes)
                .ok_or_else(|| "LVZ image decode-work aggregate overflow".to_string())?,
            object_count: self
                .object_count
                .checked_add(other.object_count)
                .ok_or_else(|| "LVZ object count overflow".to_string())?,
            image_count: self
                .image_count
                .checked_add(other.image_count)
                .ok_or_else(|| "LVZ image count overflow".to_string())?,
            reference_count: self
                .reference_count
                .checked_add(other.reference_count)
                .ok_or_else(|| "LVZ reference count overflow".to_string())?,
            texture_count: self
                .texture_count
                .checked_add(other.texture_count)
                .ok_or_else(|| "LVZ texture count overflow".to_string())?,
            chat_sound_count: self
                .chat_sound_count
                .checked_add(other.chat_sound_count)
                .ok_or_else(|| "LVZ chat sound count overflow".to_string())?,
            chat_sound_pcm_bytes: self
                .chat_sound_pcm_bytes
                .checked_add(other.chat_sound_pcm_bytes)
                .ok_or_else(|| "LVZ chat sound PCM size overflow".to_string())?,
        };
        usage.validate()?;
        Ok(usage)
    }

    pub(crate) fn validate(self) -> Result<(), String> {
        if self.object_count > MAX_LVZ_OBJECTS_PER_ZONE {
            return Err(format!(
                "LVZ retains {} objects; maximum is {MAX_LVZ_OBJECTS_PER_ZONE}",
                self.object_count
            ));
        }
        if self.image_count > MAX_LVZ_IMAGES_PER_ZONE {
            return Err(format!(
                "LVZ retains {} image definitions; maximum is {MAX_LVZ_IMAGES_PER_ZONE}",
                self.image_count
            ));
        }
        if self.reference_count > MAX_LVZ_OBJECT_REFERENCES_PER_ZONE {
            return Err(format!(
                "LVZ retains {} object/image references; maximum is {MAX_LVZ_OBJECT_REFERENCES_PER_ZONE}",
                self.reference_count
            ));
        }
        if self.texture_count > MAX_LVZ_TEXTURES_PER_ZONE {
            return Err(format!(
                "LVZ retains {} render textures; maximum is {MAX_LVZ_TEXTURES_PER_ZONE}",
                self.texture_count
            ));
        }
        if self.chat_sound_count > crate::zone_wire::MAX_LVZ_CHAT_SOUNDS_PER_ZONE {
            return Err(format!(
                "LVZ retains {} chat sounds; maximum is {}",
                self.chat_sound_count,
                crate::zone_wire::MAX_LVZ_CHAT_SOUNDS_PER_ZONE
            ));
        }
        if self.chat_sound_pcm_bytes > crate::zone_wire::MAX_LVZ_CHAT_SOUND_PCM_BYTES {
            return Err(format!(
                "LVZ chat sounds expand to {} PCM bytes; maximum is {}",
                self.chat_sound_pcm_bytes,
                crate::zone_wire::MAX_LVZ_CHAT_SOUND_PCM_BYTES
            ));
        }
        if self.pixel_bytes > MAX_ZONE_RENDER_PIXEL_BYTES {
            return Err(format!(
                "LVZ images expand to {} RGBA bytes; maximum is {MAX_ZONE_RENDER_PIXEL_BYTES}",
                self.pixel_bytes
            ));
        }
        if self.image_decode_work_bytes > MAX_ZONE_IMAGE_DECODE_WORK_BYTES {
            return Err(format!(
                "LVZ image decode work is {} RGBA bytes; maximum is {MAX_ZONE_IMAGE_DECODE_WORK_BYTES}",
                self.image_decode_work_bytes
            ));
        }
        let retained = self.retained_bytes()?;
        if retained > MAX_ZONE_RENDER_BYTES {
            return Err(format!(
                "LVZ retained render representation is {retained} bytes; maximum is {MAX_ZONE_RENDER_BYTES}"
            ));
        }
        Ok(())
    }
}

impl LvzArchive {
    pub(crate) const MAGIC_CONT: u32 = u32::from_le_bytes(*b"CONT");
    pub(crate) const MAGIC_CLV1: u32 = u32::from_le_bytes(*b"CLV1");
    pub(crate) const MAGIC_CLV2: u32 = u32::from_le_bytes(*b"CLV2");
    const HEADER_SIZE: usize = 8;
    const SECTION_HEADER_SIZE: usize = 16;
    const OBJECT_SECTION_HEADER_SIZE: usize = 12;
    const OBJECT_DEFINITION_SIZE: usize = 10;
    const IMAGE_DEFINITION_SIZE: usize = 6;
    const MAX_SECTION_COUNT: usize = 4_096;
    pub(crate) const MAX_ARCHIVE_DECODED_BYTES: usize = 16 * 1024 * 1024;

    pub(crate) fn parse(data: &[u8]) -> Result<Self, String> {
        let mut archive = Self::parse_structural(data)?;
        archive.seal_full_renderability()?;
        Ok(archive)
    }

    /// Parse/decompress every section and calculate bounded representation +
    /// image-header work without invoking a pixel decoder. Callers admitting
    /// multiple archives must aggregate `render_usage()` for all candidates
    /// before sealing any of them.
    pub(crate) fn parse_structural(data: &[u8]) -> Result<Self, String> {
        if data.len() < Self::HEADER_SIZE {
            return Err("LVZ header is too short".to_string());
        }
        if read_u32(data, 0)? != Self::MAGIC_CONT {
            return Err("LVZ magic is not CONT".to_string());
        }

        let section_count = read_u32(data, 4)? as usize;
        if section_count > Self::MAX_SECTION_COUNT {
            return Err(format!(
                "LVZ declares {section_count} sections; maximum is {}",
                Self::MAX_SECTION_COUNT
            ));
        }
        let mut offset = Self::HEADER_SIZE;
        let mut decoded_bytes = 0_usize;
        let mut descriptors = Vec::with_capacity(section_count);

        for _ in 0..section_count {
            if offset
                .checked_add(Self::SECTION_HEADER_SIZE)
                .is_none_or(|end| end > data.len())
            {
                return Err("unexpected EOF reading LVZ section header".to_string());
            }
            if read_u32(data, offset)? != Self::MAGIC_CONT {
                return Err("LVZ section magic is not CONT".to_string());
            }
            let decompressed_size = read_u32(data, offset + 4)? as usize;
            if decompressed_size > Self::MAX_ARCHIVE_DECODED_BYTES {
                return Err(format!(
                    "LVZ section expands to {decompressed_size} bytes; maximum is {}",
                    Self::MAX_ARCHIVE_DECODED_BYTES
                ));
            }
            decoded_bytes = decoded_bytes
                .checked_add(decompressed_size)
                .ok_or_else(|| "LVZ aggregate decoded size overflow".to_string())?;
            if decoded_bytes > Self::MAX_ARCHIVE_DECODED_BYTES {
                return Err(format!(
                    "LVZ archive expands to {decoded_bytes} bytes; aggregate maximum is {}",
                    Self::MAX_ARCHIVE_DECODED_BYTES
                ));
            }
            let timestamp = read_u32(data, offset + 8)?;
            let compressed_size = read_u32(data, offset + 12)? as usize;
            offset += Self::SECTION_HEADER_SIZE;

            let (filename, next_offset) = read_bounded_lvz_filename(data, offset)?;
            offset = next_offset;
            let section_end = offset
                .checked_add(compressed_size)
                .ok_or_else(|| "LVZ section size overflow".to_string())?;
            if section_end > data.len() {
                return Err("unexpected EOF reading LVZ section data".to_string());
            }

            let raw = &data[offset..section_end];
            offset = section_end;
            descriptors.push(LvzSectionDescriptor {
                filename,
                timestamp,
                decompressed_size,
                compressed_size,
                raw,
            });
        }

        if offset != data.len() {
            return Err(format!(
                "LVZ contains {} trailing bytes after its declared sections",
                data.len() - offset
            ));
        }

        // Only decode after every header, range, trailing-byte condition, and
        // the archive-wide expansion budget has passed. A late oversized
        // declaration can therefore never leave earlier expanded sections
        // retained while the parser discovers the failure.
        let mut sections = Vec::with_capacity(section_count);
        let mut names = BTreeSet::new();
        for descriptor in descriptors {
            let section_data = lvz_section_data(
                descriptor.raw,
                descriptor.decompressed_size,
                descriptor.compressed_size,
            )?;
            let object_section = descriptor.timestamp == 0 && descriptor.filename.is_empty();
            let filename = intern_lvz_filename(descriptor.filename, object_section, &mut names)?;
            let kind = if object_section {
                LvzSectionKind::Objects(LvzObjectSection::parse_with_names(
                    &section_data,
                    &mut names,
                )?)
            } else {
                LvzSectionKind::File
            };
            sections.push(LvzSection {
                filename,
                timestamp: descriptor.timestamp,
                data: section_data.into(),
                kind,
            });
        }

        let archive = Self {
            sections,
            fully_validated: false,
            prepared_images: None,
        };
        // Account the complete representation and all payload header-derived
        // decode work. Full pixel/audio validation is intentionally deferred
        // until an aggregate caller has preflighted every archive.
        archive.render_usage()?;
        Ok(archive)
    }

    pub(crate) fn seal_full_renderability(&mut self) -> Result<(), String> {
        self.seal_with_prepared_images(&mut crate::render_limits::ImagePreparationBudget::default())
    }

    pub(crate) fn seal_with_prepared_images(
        &mut self,
        budget: &mut crate::render_limits::ImagePreparationBudget,
    ) -> Result<(), String> {
        if self.fully_validated {
            return Ok(());
        }
        // Every payload validates, including overwritten duplicates. Only the
        // last section per normalized name is useful to the renderer. The
        // server/default path retains no pixels and needs no index.
        let mut last_sections = BTreeMap::new();
        if budget.available() {
            for (index, section) in self.sections.iter().enumerate() {
                if matches!(section.kind, LvzSectionKind::File) {
                    last_sections.insert(Arc::clone(&section.filename), index);
                }
            }
        }
        let mut prepared: Option<Vec<Option<crate::bm2::DecodedImage>>> = None;
        for (index, section) in self.sections.iter().enumerate() {
            if !matches!(section.kind, LvzSectionKind::File) {
                continue;
            }
            if let Some(decoded) =
                crate::render_limits::decode_image_for_filename(&section.filename, &section.data)?
                && last_sections.get(&section.filename) == Some(&index)
            {
                // Charge the complete slot vector (not just occupied slots),
                // Arc/Mutex/allocation bookkeeping, and actual Vec capacity
                // before retaining the first decoded buffer. One budget is
                // consumed across all archives and is never refilled here.
                let slots_bytes = if prepared.is_none() {
                    self.sections.len() * std::mem::size_of::<Option<crate::bm2::DecodedImage>>()
                        + PREPARED_ARCHIVE_BOOKKEEPING_BYTES
                } else {
                    0
                };
                if let Some(bytes) = decoded.rgba.capacity().checked_add(slots_bytes)
                    && budget.try_reserve(bytes)
                {
                    let slots = prepared.get_or_insert_with(|| {
                        let mut slots = Vec::with_capacity(self.sections.len());
                        slots.resize_with(self.sections.len(), || None);
                        slots
                    });
                    slots[index] = Some(decoded);
                }
            }
            crate::render_limits::validate_chat_sound_for_filename(
                &section.filename,
                &section.data,
            )?;
        }
        self.prepared_images = prepared.map(|slots| PreparedLvzImages(Arc::new(Mutex::new(slots))));
        self.fully_validated = true;
        Ok(())
    }

    #[allow(dead_code)] // server catalog does not install/render archives
    pub(crate) fn sections(&self) -> &[LvzSection] {
        &self.sections
    }

    #[cfg(test)]
    pub(crate) fn into_sections(self) -> Vec<LvzSection> {
        self.sections
    }

    #[allow(dead_code)] // server catalog does not install/render archives
    pub(crate) fn take_prepared_image(&self, index: usize) -> Option<crate::bm2::DecodedImage> {
        self.prepared_images
            .as_ref()?
            .0
            .lock()
            .expect("prepared image lock is not poisoned")
            .get_mut(index)?
            .take()
    }

    #[allow(dead_code)] // server catalog does not install/render archives
    pub(crate) fn discard_prepared_images(&self) {
        if let Some(prepared) = &self.prepared_images {
            *prepared
                .0
                .lock()
                .expect("prepared image lock is not poisoned") = Vec::new();
        }
    }

    pub(crate) fn require_full_validation_seal(&self) -> Result<(), String> {
        if self.fully_validated {
            Ok(())
        } else {
            Err("LVZ archive has not passed full render/audio validation".to_string())
        }
    }

    #[cfg(test)]
    pub(crate) fn unvalidated_for_usage_test(sections: Vec<LvzSection>) -> Self {
        Self {
            sections,
            fully_validated: false,
            prepared_images: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn validated_for_test(sections: Vec<LvzSection>) -> Result<Self, String> {
        let mut archive = Self::unvalidated_for_usage_test(sections);
        archive.render_usage()?;
        archive.seal_full_renderability()?;
        Ok(archive)
    }

    pub(crate) fn object_sections(&self) -> impl Iterator<Item = &LvzObjectSection> {
        self.sections
            .iter()
            .filter_map(|section| match &section.kind {
                LvzSectionKind::Objects(objects) => Some(objects),
                LvzSectionKind::File => None,
            })
    }

    pub(crate) fn decoded_bytes(&self) -> usize {
        self.sections.iter().map(|section| section.data.len()).sum()
    }

    pub(crate) fn render_usage(&self) -> Result<LvzRenderUsage, String> {
        let mut usage = LvzRenderUsage {
            decoded_source_bytes: self.decoded_bytes(),
            ..LvzRenderUsage::default()
        };
        // File sections are last-wins by their canonical name. Account every
        // retained section/name below, but account only the final texture for
        // each key as pixel/GPU state. `parse` separately performs a bounded
        // full decode of every payload, including overwritten duplicates.
        let mut render_images = BTreeMap::<Arc<str>, usize>::new();
        for section in &self.sections {
            match &section.kind {
                LvzSectionKind::File => {
                    usage.metadata_bytes = usage
                        .metadata_bytes
                        .checked_add(LVZ_FILE_RETAINED_BASE_BYTES)
                        .and_then(|bytes| bytes.checked_add(section.filename.len()))
                        .ok_or_else(|| "LVZ file metadata size overflow".to_string())?;
                    if let Some(allocation) =
                        image_allocation_for_filename(&section.filename, &section.data)?
                    {
                        usage.image_decode_work_bytes = usage
                            .image_decode_work_bytes
                            .checked_add(allocation.rgba_bytes)
                            .ok_or_else(|| {
                                "LVZ image decode-work aggregate overflow".to_string()
                            })?;
                        render_images.insert(Arc::clone(&section.filename), allocation.rgba_bytes);
                        usage.metadata_bytes = usage
                            .metadata_bytes
                            .checked_add(LVZ_TEXTURE_RETAINED_BASE_BYTES)
                            .ok_or_else(|| "LVZ texture metadata size overflow".to_string())?;
                    }
                    if let Some(pcm_bytes) = crate::render_limits::validate_chat_sound_for_filename(
                        &section.filename,
                        &section.data,
                    )? {
                        usage.chat_sound_count = usage
                            .chat_sound_count
                            .checked_add(1)
                            .ok_or_else(|| "LVZ chat sound count overflow".to_string())?;
                        usage.chat_sound_pcm_bytes = usage
                            .chat_sound_pcm_bytes
                            .checked_add(pcm_bytes)
                            .ok_or_else(|| "LVZ chat sound PCM size overflow".to_string())?;
                    }
                }
                LvzSectionKind::Objects(objects) => {
                    usage.object_count = usage
                        .object_count
                        .checked_add(objects.objects.len())
                        .ok_or_else(|| "LVZ object count overflow".to_string())?;
                    usage.reference_count = usage
                        .reference_count
                        .checked_add(objects.objects.len())
                        .ok_or_else(|| "LVZ reference count overflow".to_string())?;
                    usage.image_count = usage
                        .image_count
                        .checked_add(objects.images.len())
                        .ok_or_else(|| "LVZ image count overflow".to_string())?;
                    let object_bytes = objects
                        .objects
                        .len()
                        .checked_mul(
                            LVZ_OBJECT_RETAINED_BYTES + LVZ_RENDER_REFERENCE_RETAINED_BYTES,
                        )
                        .ok_or_else(|| "LVZ object metadata size overflow".to_string())?;
                    let image_bytes = objects
                        .images
                        .iter()
                        .try_fold(0_usize, |total, image| {
                            LVZ_IMAGE_RETAINED_BASE_BYTES
                                .checked_add(LVZ_FILENAME_REFERENCE_RETAINED_BYTES)
                                .and_then(|bytes| bytes.checked_add(image.filename.len()))
                                .and_then(|bytes| total.checked_add(bytes))
                        })
                        .ok_or_else(|| "LVZ image metadata size overflow".to_string())?;
                    usage.metadata_bytes = usage
                        .metadata_bytes
                        .checked_add(object_bytes)
                        .and_then(|total| total.checked_add(image_bytes))
                        .ok_or_else(|| "LVZ metadata aggregate overflow".to_string())?;
                }
            }
            usage.texture_count = render_images.len();
            usage.pixel_bytes = render_images
                .values()
                .try_fold(0_usize, |total, bytes| total.checked_add(*bytes))
                .ok_or_else(|| "LVZ pixel aggregate overflow".to_string())?;
            usage.validate()?;
        }
        Ok(usage)
    }

    pub(crate) fn render_image_allocations(&self) -> Result<Vec<(Arc<str>, usize)>, String> {
        let mut images = Vec::new();
        for section in &self.sections {
            if !matches!(section.kind, LvzSectionKind::File) {
                continue;
            }
            if let Some(allocation) =
                image_allocation_for_filename(&section.filename, &section.data)?
            {
                images.push((Arc::clone(&section.filename), allocation.rgba_bytes));
            }
        }
        Ok(images)
    }
}

fn read_bounded_lvz_filename(data: &[u8], offset: usize) -> Result<(&str, usize), String> {
    if offset > data.len() {
        return Err("unexpected EOF reading LVZ filename".to_string());
    }
    let remaining = &data[offset..];
    let relative_end = remaining
        .iter()
        .take(MAX_LVZ_FILENAME_BYTES + 1)
        .position(|byte| *byte == 0)
        .ok_or_else(|| {
            if remaining.len() > MAX_LVZ_FILENAME_BYTES {
                format!("LVZ filename exceeds {MAX_LVZ_FILENAME_BYTES} bytes")
            } else {
                "unterminated LVZ filename".to_string()
            }
        })?;
    let raw = std::str::from_utf8(&remaining[..relative_end])
        .map_err(|_| "LVZ filename is not valid UTF-8".to_string())?;
    if !raw.is_ascii() {
        return Err("LVZ filename is not ASCII".to_string());
    }
    Ok((raw, offset + relative_end + 1))
}

fn intern_lvz_filename(
    raw: &str,
    allow_empty: bool,
    names: &mut BTreeSet<Arc<str>>,
) -> Result<Arc<str>, String> {
    if raw.is_empty() {
        if allow_empty {
            return Ok(Arc::from(""));
        }
        return Err("LVZ file section has an empty filename".to_string());
    }
    let mut canonical = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        let byte = match byte {
            b'\\' => b'/',
            b'A'..=b'Z' => byte + (b'a' - b'A'),
            b'!'..=b'~' => byte,
            b' '..=b'~' => byte,
            _ => return Err(format!("LVZ filename contains a control byte: {raw:?}")),
        };
        canonical.push(byte as char);
    }
    if canonical.starts_with('/')
        || canonical.ends_with('/')
        || canonical
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!(
            "LVZ filename is not a canonical relative asset name: {raw:?}"
        ));
    }
    if let Some(existing) = names.get(canonical.as_str()) {
        return Ok(Arc::clone(existing));
    }
    let canonical: Arc<str> = Arc::from(canonical);
    names.insert(Arc::clone(&canonical));
    Ok(canonical)
}

fn lvz_section_data(
    raw: &[u8],
    decompressed_size: usize,
    compressed_size: usize,
) -> Result<Vec<u8>, String> {
    if decompressed_size == compressed_size {
        if let Ok(decoded) = decompress_zlib_payload_bounded(raw, decompressed_size) {
            if decoded.len() == decompressed_size {
                return Ok(decoded);
            }
        }
        return Ok(raw.to_vec());
    }

    let decoded = decompress_zlib_payload_bounded(raw, decompressed_size)?;
    if decoded.len() != decompressed_size {
        return Err(format!(
            "LVZ section decompressed to {} bytes, expected {decompressed_size}",
            decoded.len()
        ));
    }
    Ok(decoded)
}

impl LvzObjectDefinition {
    fn parse(data: &[u8], offset: usize, version: u32) -> Result<Self, String> {
        if offset + LvzArchive::OBJECT_DEFINITION_SIZE > data.len() {
            return Err("unexpected EOF reading LVZ object definition".to_string());
        }

        let id_bits = read_u16(data, offset)?;
        let x_bits = read_u16(data, offset + 2)?;
        let y_bits = read_u16(data, offset + 4)?;
        let display_bits = read_u16(data, offset + 8)?;

        let map_object = id_bits & 1 != 0;
        let (x_screen, y_screen, x_type, y_type) =
            if !map_object && version == LvzArchive::MAGIC_CLV1 {
                (x_bits as i16, y_bits as i16, 0, 0)
            } else {
                (
                    sign_extend_i16(x_bits >> 4, 12),
                    sign_extend_i16(y_bits >> 4, 12),
                    (x_bits & 0x000f) as u8,
                    (y_bits & 0x000f) as u8,
                )
            };

        Ok(Self {
            map_object,
            object_id: id_bits >> 1,
            x_map: x_bits as i16,
            y_map: y_bits as i16,
            x_screen,
            y_screen,
            x_type,
            y_type,
            image_number: data[offset + 6],
            layer: data[offset + 7],
            display_time: display_bits & 0x0fff,
            display_mode: (display_bits >> 12) as u8,
        })
    }
}

impl LvzObjectSection {
    #[cfg(test)]
    fn parse(data: &[u8]) -> Result<Self, String> {
        let mut names = BTreeSet::new();
        Self::parse_with_names(data, &mut names)
    }

    fn parse_with_names(data: &[u8], names: &mut BTreeSet<Arc<str>>) -> Result<Self, String> {
        if data.len() < LvzArchive::OBJECT_SECTION_HEADER_SIZE {
            return Err("LVZ object section header is too short".to_string());
        }

        let version = read_u32(data, 0)?;
        if version != LvzArchive::MAGIC_CLV1 && version != LvzArchive::MAGIC_CLV2 {
            return Err("LVZ object section magic is not CLV1/CLV2".to_string());
        }

        let object_count = read_u32(data, 4)? as usize;
        let image_count = read_u32(data, 8)? as usize;
        if object_count > MAX_LVZ_OBJECTS_PER_ZONE {
            return Err(format!(
                "LVZ object section declares {object_count} objects; maximum is {MAX_LVZ_OBJECTS_PER_ZONE}"
            ));
        }
        if image_count > 256 || image_count > MAX_LVZ_IMAGES_PER_ZONE {
            return Err(format!(
                "LVZ object section declares {image_count} images; maximum addressable count is 256"
            ));
        }
        let object_start = LvzArchive::OBJECT_SECTION_HEADER_SIZE;
        let image_start = object_count
            .checked_mul(LvzArchive::OBJECT_DEFINITION_SIZE)
            .and_then(|size| object_start.checked_add(size))
            .ok_or_else(|| "LVZ object definition size overflow".to_string())?;
        if image_start > data.len() {
            return Err("unexpected EOF reading LVZ object definitions".to_string());
        }
        let remaining = data.len() - image_start;
        if image_count > remaining / (LvzArchive::IMAGE_DEFINITION_SIZE + 1) {
            return Err("LVZ image count cannot fit in object section".to_string());
        }

        let mut objects = Vec::with_capacity(object_count);
        for index in 0..object_count {
            let offset = object_start + index * LvzArchive::OBJECT_DEFINITION_SIZE;
            let object = LvzObjectDefinition::parse(data, offset, version)?;
            if object.image_number as usize >= image_count {
                return Err(format!(
                    "LVZ object {} references image {}, but only {image_count} images are declared",
                    object.object_id, object.image_number
                ));
            }
            objects.push(object);
        }

        let mut images = Vec::with_capacity(image_count);
        let mut offset = image_start;
        for _ in 0..image_count {
            if offset
                .checked_add(LvzArchive::IMAGE_DEFINITION_SIZE)
                .is_none_or(|end| end > data.len())
            {
                return Err("unexpected EOF reading LVZ image definition".to_string());
            }
            let x_count = read_i16(data, offset)?;
            let y_count = read_i16(data, offset + 2)?;
            let animation_time = read_i16(data, offset + 4)?;
            offset += LvzArchive::IMAGE_DEFINITION_SIZE;
            let (filename, next_offset) = read_bounded_lvz_filename(data, offset)?;
            offset = next_offset;
            images.push(LvzImageDefinition {
                x_count,
                y_count,
                animation_time,
                filename: intern_lvz_filename(filename, false, names)?,
            });
        }

        if offset != data.len() {
            return Err(format!(
                "LVZ object section contains {} trailing bytes",
                data.len() - offset
            ));
        }

        Ok(Self {
            version,
            objects,
            images,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    fn one_pixel_png(color: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&color, 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    #[test]
    fn prepared_images_validate_every_duplicate_and_transfer_only_last_wins_once() {
        let source = archive_with_raw_sections(&[
            (b"DUP.PNG", 1, one_pixel_png([1, 2, 3, 4])),
            (b"dup.png", 2, one_pixel_png([5, 6, 7, 8])),
            (b"other.png", 3, one_pixel_png([9, 10, 11, 12])),
        ]);
        let mut archive = LvzArchive::parse_structural(&source).unwrap();
        crate::render_limits::reset_full_image_validation_calls();
        let mut budget = crate::render_limits::ImagePreparationBudget::for_zone_high_water(0);
        archive.seal_with_prepared_images(&mut budget).unwrap();
        assert_eq!(crate::render_limits::full_image_validation_calls(), 3);
        archive.require_full_validation_seal().unwrap();
        let alias = archive.clone();
        assert!(
            archive.take_prepared_image(0).is_none(),
            "overwritten pixels are not retained"
        );
        let final_image = archive.take_prepared_image(1).unwrap();
        assert_eq!((final_image.width, final_image.height), (1, 1));
        assert_eq!(final_image.rgba, [5, 6, 7, 8]);
        assert!(
            alias.take_prepared_image(1).is_none(),
            "clones share one consumable handoff"
        );
        assert_eq!(archive, alias, "consumption cannot change archive identity");
        archive.discard_prepared_images();
        assert!(alias.take_prepared_image(2).is_none());
        assert_eq!(
            archive
                .prepared_images
                .as_ref()
                .unwrap()
                .0
                .lock()
                .unwrap()
                .capacity(),
            0
        );
        archive.seal_with_prepared_images(&mut budget).unwrap();
        assert_eq!(
            crate::render_limits::full_image_validation_calls(),
            3,
            "seal/consumption never decode again"
        );
    }

    #[test]
    fn prepared_images_exhaustion_keeps_validation_and_does_not_renew_per_archive() {
        let source = archive_with_raw_sections(&[(b"one.png", 1, one_pixel_png([1, 2, 3, 4]))]);
        let mut first = LvzArchive::parse_structural(&source).unwrap();
        let mut second = first.clone();
        let one_image_bytes = std::mem::size_of::<Option<crate::bm2::DecodedImage>>()
            + PREPARED_ARCHIVE_BOOKKEEPING_BYTES
            + 4;
        let reserved_work = crate::zone_wire::MAX_IMAGE_DECODER_ALLOC_BYTES as usize
            + 2 * crate::zone_wire::MAX_RENDER_IMAGE_RGBA_BYTES;
        let mut budget = crate::render_limits::ImagePreparationBudget::for_zone_high_water(
            crate::zone_wire::MAX_ZONE_RENDER_PEAK_BYTES - reserved_work - one_image_bytes,
        );
        crate::render_limits::reset_full_image_validation_calls();
        first.seal_with_prepared_images(&mut budget).unwrap();
        assert!(!budget.available());
        second.seal_with_prepared_images(&mut budget).unwrap();
        assert_eq!(crate::render_limits::full_image_validation_calls(), 2);
        assert_eq!(first.take_prepared_image(0).unwrap().rgba, [1, 2, 3, 4]);
        assert!(second.take_prepared_image(0).is_none());
        second.require_full_validation_seal().unwrap();
        assert!(
            !budget.available(),
            "consumption does not refill the zone's reservation"
        );
        let section = &second.sections()[0];
        let fallback =
            crate::render_limits::decode_image_for_filename(&section.filename, &section.data)
                .unwrap()
                .unwrap();
        assert_eq!(fallback.rgba, [1, 2, 3, 4]);
        assert_eq!(crate::render_limits::full_image_validation_calls(), 3);
    }

    #[test]
    fn prepared_images_reject_malformed_overwritten_payload_and_publish_no_partial_cache() {
        let valid = include_bytes!("../../assets/graphics/bullets.bm2");
        let source = archive_with_raw_sections(&[
            (b"first.png", 1, one_pixel_png([1, 2, 3, 4])),
            (b"dup.bm2", 2, valid[..54].to_vec()),
            (b"dup.bm2", 3, valid.to_vec()),
        ]);
        let mut archive = LvzArchive::parse_structural(&source).unwrap();
        let mut budget = crate::render_limits::ImagePreparationBudget::for_zone_high_water(0);
        assert!(archive.seal_with_prepared_images(&mut budget).is_err());
        assert!(archive.require_full_validation_seal().is_err());
        assert!(archive.prepared_images.is_none());
        assert!(archive.take_prepared_image(0).is_none());
    }

    fn object_section(object_count: usize, image_count: usize) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&LvzArchive::MAGIC_CLV1.to_le_bytes());
        data.extend_from_slice(&(object_count as u32).to_le_bytes());
        data.extend_from_slice(&(image_count as u32).to_le_bytes());
        data.resize(12 + object_count * LvzArchive::OBJECT_DEFINITION_SIZE, 0);
        for _ in 0..image_count {
            data.extend_from_slice(&[1, 0, 1, 0, 0, 0]);
            data.extend_from_slice(b"a.bm2\0");
        }
        data
    }

    fn archive_with_object_sections(sections: &[Vec<u8>]) -> Vec<u8> {
        let mut archive = Vec::new();
        archive.extend_from_slice(&LvzArchive::MAGIC_CONT.to_le_bytes());
        archive.extend_from_slice(&(sections.len() as u32).to_le_bytes());
        for section in sections {
            archive.extend_from_slice(&LvzArchive::MAGIC_CONT.to_le_bytes());
            archive.extend_from_slice(&(section.len() as u32).to_le_bytes());
            archive.extend_from_slice(&0_u32.to_le_bytes());
            archive.extend_from_slice(&(section.len() as u32).to_le_bytes());
            archive.push(0);
            archive.extend_from_slice(section);
        }
        archive
    }

    fn archive_with_raw_sections(sections: &[(&[u8], u32, Vec<u8>)]) -> Vec<u8> {
        let mut archive = Vec::new();
        archive.extend_from_slice(&LvzArchive::MAGIC_CONT.to_le_bytes());
        archive.extend_from_slice(&(sections.len() as u32).to_le_bytes());
        for (filename, timestamp, section) in sections {
            archive.extend_from_slice(&LvzArchive::MAGIC_CONT.to_le_bytes());
            archive.extend_from_slice(&(section.len() as u32).to_le_bytes());
            archive.extend_from_slice(&timestamp.to_le_bytes());
            archive.extend_from_slice(&(section.len() as u32).to_le_bytes());
            archive.extend_from_slice(filename);
            archive.push(0);
            archive.extend_from_slice(section);
        }
        archive
    }

    #[test]
    fn object_count_accepts_exact_limit_and_rejects_one_more_before_allocation() {
        assert!(LvzObjectSection::parse(&object_section(MAX_LVZ_OBJECTS_PER_ZONE, 1)).is_ok());
        let mut over = object_section(0, 0);
        over[4..8].copy_from_slice(&((MAX_LVZ_OBJECTS_PER_ZONE + 1) as u32).to_le_bytes());
        assert!(LvzObjectSection::parse(&over).is_err());
    }

    #[test]
    fn image_count_and_references_are_bounded_and_resolved_during_parse() {
        assert!(LvzObjectSection::parse(&object_section(0, 256)).is_ok());
        let mut over = object_section(0, 0);
        over[8..12].copy_from_slice(&257_u32.to_le_bytes());
        assert!(LvzObjectSection::parse(&over).is_err());
        assert!(LvzObjectSection::parse(&object_section(1, 0)).is_err());
    }

    #[test]
    fn archive_rejects_aggregate_object_fanout_across_sections() {
        let archive = archive_with_object_sections(&[
            object_section(MAX_LVZ_OBJECTS_PER_ZONE / 2 + 1, 1),
            object_section(MAX_LVZ_OBJECTS_PER_ZONE / 2, 1),
        ]);
        assert!(LvzArchive::parse(&archive).is_err());
    }

    #[test]
    fn filenames_are_strict_bounded_canonical_and_interned_without_utf8_expansion() {
        let invalid_utf8 = archive_with_raw_sections(&[(&[0xff, 0xfe], 1, vec![1])]);
        assert!(LvzArchive::parse(&invalid_utf8).is_err());

        let long = vec![b'a'; MAX_LVZ_FILENAME_BYTES + 1];
        let overlong = archive_with_raw_sections(&[(&long, 1, vec![1])]);
        assert!(LvzArchive::parse(&overlong).is_err());

        let mut objects = object_section(0, 1);
        let filename_start = objects.len() - b"a.bm2\0".len();
        objects.truncate(filename_start);
        objects.extend_from_slice(b"dir/foo.bin\0");
        let archive =
            archive_with_raw_sections(&[(b"DIR\\Foo.BIN", 1, vec![1]), (b"", 0, objects)]);
        let parsed = LvzArchive::parse(&archive).unwrap();
        let image = &parsed.object_sections().next().unwrap().images[0].filename;
        assert_eq!(parsed.sections[0].filename.as_ref(), "dir/foo.bin");
        assert!(Arc::ptr_eq(&parsed.sections[0].filename, image));
    }

    #[test]
    fn every_file_filename_and_tree_reference_is_charged_even_when_not_renderable() {
        let name: Arc<str> = Arc::from("notes/read me.txt");
        let archive = LvzArchive::unvalidated_for_usage_test(vec![
            LvzSection {
                filename: Arc::clone(&name),
                timestamp: 1,
                data: Arc::from([1_u8]),
                kind: LvzSectionKind::File,
            },
            LvzSection {
                filename: Arc::clone(&name),
                timestamp: 2,
                data: Arc::from([2_u8]),
                kind: LvzSectionKind::File,
            },
        ]);
        let usage = archive.render_usage().unwrap();
        assert_eq!(
            usage.metadata_bytes,
            2 * (LVZ_FILE_RETAINED_BASE_BYTES + name.len())
        );
    }

    fn texture_archive(count: usize) -> LvzArchive {
        let mut bm2 = vec![0_u8; 54];
        bm2[..2].copy_from_slice(b"BM");
        bm2[18..22].copy_from_slice(&1_i32.to_le_bytes());
        bm2[22..26].copy_from_slice(&1_i32.to_le_bytes());
        bm2[26..28].copy_from_slice(&1_u16.to_le_bytes());
        bm2[28..30].copy_from_slice(&24_u16.to_le_bytes());
        LvzArchive::unvalidated_for_usage_test(
            (0..count)
                .map(|index| LvzSection {
                    filename: format!("t{index}.bm2").into(),
                    timestamp: 1,
                    data: Arc::from(bm2.clone()),
                    kind: LvzSectionKind::File,
                })
                .collect(),
        )
    }

    #[test]
    fn aggregate_texture_count_accepts_exact_limit_and_rejects_one_more() {
        let half = texture_archive(MAX_LVZ_TEXTURES_PER_ZONE / 2)
            .render_usage()
            .unwrap();
        let exact = half.checked_add(half).unwrap();
        assert_eq!(exact.texture_count, MAX_LVZ_TEXTURES_PER_ZONE);
        assert!(
            exact
                .checked_add(texture_archive(1).render_usage().unwrap())
                .is_err()
        );
    }
}
