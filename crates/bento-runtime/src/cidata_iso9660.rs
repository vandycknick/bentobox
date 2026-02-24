use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use chrono::{Datelike, Timelike, Utc};

const SECTOR_SIZE: usize = 2048;
const SYSTEM_AREA_SECTORS: u32 = 16;

#[derive(Debug, Clone)]
pub struct CidataEntry {
    pub name: String,
    pub contents: Vec<u8>,
}

pub fn write_cidata_iso(
    output_path: &Path,
    volume_label: &str,
    entries: &[CidataEntry],
) -> std::io::Result<()> {
    let root_dir_lba = SYSTEM_AREA_SECTORS + 4;

    let root_dir_length_preview = estimated_root_directory_len(entries);
    let root_dir_sectors = div_ceil(root_dir_length_preview, SECTOR_SIZE) as u32;
    let first_file_lba = root_dir_lba + root_dir_sectors;

    let mut file_lbas = Vec::with_capacity(entries.len());
    let mut next_lba = first_file_lba;
    for entry in entries {
        file_lbas.push(next_lba);
        next_lba += div_ceil(entry.contents.len(), SECTOR_SIZE) as u32;
    }

    let root_dir_bytes =
        build_root_directory_bytes(root_dir_lba, root_dir_lba, entries, &file_lbas);
    let volume_space_size = next_lba;

    let mut file = File::create(output_path)?;

    write_zeroed_sectors(&mut file, 0, SYSTEM_AREA_SECTORS)?;

    let pvd = build_primary_volume_descriptor(
        volume_label,
        volume_space_size,
        root_dir_lba,
        root_dir_bytes.len() as u32,
        SYSTEM_AREA_SECTORS + 2,
        SYSTEM_AREA_SECTORS + 3,
    );
    write_sector_at(&mut file, SYSTEM_AREA_SECTORS, &pvd)?;

    let terminator = build_volume_terminator_descriptor();
    write_sector_at(&mut file, SYSTEM_AREA_SECTORS + 1, &terminator)?;

    let path_table_le = build_root_path_table(true, root_dir_lba);
    let path_table_be = build_root_path_table(false, root_dir_lba);
    write_sector_at(&mut file, SYSTEM_AREA_SECTORS + 2, &path_table_le)?;
    write_sector_at(&mut file, SYSTEM_AREA_SECTORS + 3, &path_table_be)?;

    write_at_lba(&mut file, root_dir_lba, &root_dir_bytes)?;

    for (entry, lba) in entries.iter().zip(file_lbas.iter()) {
        write_at_lba(&mut file, *lba, &entry.contents)?;
    }

    Ok(())
}

fn build_primary_volume_descriptor(
    volume_label: &str,
    volume_space_size: u32,
    root_dir_lba: u32,
    root_dir_data_len: u32,
    path_table_le_lba: u32,
    path_table_be_lba: u32,
) -> [u8; SECTOR_SIZE] {
    let mut pvd = [0u8; SECTOR_SIZE];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;

    write_ascii_padded(&mut pvd[8..40], "BENTO");
    write_ascii_padded(&mut pvd[40..72], volume_label);
    write_u32_both_endian(&mut pvd[80..88], volume_space_size);
    write_u16_both_endian(&mut pvd[120..124], 1);
    write_u16_both_endian(&mut pvd[124..128], 1);
    write_u16_both_endian(&mut pvd[128..132], SECTOR_SIZE as u16);
    write_u32_both_endian(&mut pvd[132..140], 10);
    pvd[140..144].copy_from_slice(&path_table_le_lba.to_le_bytes());
    pvd[148..152].copy_from_slice(&path_table_be_lba.to_be_bytes());

    let root_dir_record = build_directory_record(root_dir_lba, root_dir_data_len, true, &[0]);
    pvd[156..190].copy_from_slice(&root_dir_record);

    write_ascii_padded(&mut pvd[190..318], "BENTO");
    write_ascii_padded(&mut pvd[318..446], "BENTO");
    write_ascii_padded(&mut pvd[446..574], "BENTO");
    write_ascii_padded(&mut pvd[574..702], "BENTO");

    let now = Utc::now();
    write_volume_datetime(&mut pvd[813..830], now);
    write_volume_datetime(&mut pvd[830..847], now);
    pvd[881] = 1;

    pvd
}

fn build_volume_terminator_descriptor() -> [u8; SECTOR_SIZE] {
    let mut term = [0u8; SECTOR_SIZE];
    term[0] = 255;
    term[1..6].copy_from_slice(b"CD001");
    term[6] = 1;
    term
}

fn build_root_path_table(little_endian: bool, root_dir_lba: u32) -> [u8; SECTOR_SIZE] {
    let mut data = [0u8; SECTOR_SIZE];
    data[0] = 1;
    data[1] = 0;
    if little_endian {
        data[2..6].copy_from_slice(&root_dir_lba.to_le_bytes());
        data[6..8].copy_from_slice(&1u16.to_le_bytes());
    } else {
        data[2..6].copy_from_slice(&root_dir_lba.to_be_bytes());
        data[6..8].copy_from_slice(&1u16.to_be_bytes());
    }
    data[8] = 0;
    data[9] = 0;
    data
}

fn build_root_directory_bytes(
    self_lba: u32,
    parent_lba: u32,
    entries: &[CidataEntry],
    entry_lbas: &[u32],
) -> Vec<u8> {
    let mut records = Vec::with_capacity(entries.len() + 2);
    let mut file_records = Vec::with_capacity(entries.len());

    for (entry, lba) in entries.iter().zip(entry_lbas.iter().copied()) {
        let iso_file_id = to_iso_file_id(&entry.name);
        let record = build_directory_record(lba, entry.contents.len() as u32, false, &iso_file_id);
        file_records.push(record);
    }

    let provisional_root_len = packed_len_for_records(2 + file_records.len(), &file_records);
    let self_record = build_directory_record(self_lba, provisional_root_len as u32, true, &[0]);
    let parent_record = build_directory_record(parent_lba, provisional_root_len as u32, true, &[1]);
    records.push(self_record);
    records.push(parent_record);

    records.extend(file_records);

    pack_records_to_sectors(records)
}

fn estimated_root_directory_len(entries: &[CidataEntry]) -> usize {
    let file_records: Vec<Vec<u8>> = entries
        .iter()
        .map(|entry| {
            let iso_file_id = to_iso_file_id(&entry.name);
            build_directory_record(0, entry.contents.len() as u32, false, &iso_file_id)
        })
        .collect();
    packed_len_for_records(2 + file_records.len(), &file_records)
}

fn packed_len_for_records(
    total_record_count: usize,
    records_without_dot_entries: &[Vec<u8>],
) -> usize {
    let dot_len = build_directory_record(0, 0, true, &[0]).len();
    let dotdot_len = build_directory_record(0, 0, true, &[1]).len();

    let mut records = Vec::with_capacity(total_record_count);
    records.push(vec![0; dot_len]);
    records.push(vec![0; dotdot_len]);
    records.extend(records_without_dot_entries.iter().cloned());

    pack_records_to_sectors(records).len()
}

fn to_iso_file_id(name: &str) -> Vec<u8> {
    let mut mapped = Vec::with_capacity(name.len() + 2);
    for b in name.bytes() {
        let normalized = match b {
            b'a'..=b'z' => b.to_ascii_uppercase(),
            b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' => b,
            _ => b'_',
        };
        mapped.push(normalized);
    }
    mapped.extend_from_slice(b";1");
    mapped
}

fn build_directory_record(extent_lba: u32, data_len: u32, is_dir: bool, file_id: &[u8]) -> Vec<u8> {
    let padding = if file_id.len().is_multiple_of(2) {
        1
    } else {
        0
    };
    let record_len = 33 + file_id.len() + padding;
    let mut record = vec![0u8; record_len];

    record[0] = record_len as u8;
    record[1] = 0;

    record[2..6].copy_from_slice(&extent_lba.to_le_bytes());
    record[6..10].copy_from_slice(&extent_lba.to_be_bytes());
    record[10..14].copy_from_slice(&data_len.to_le_bytes());
    record[14..18].copy_from_slice(&data_len.to_be_bytes());

    let now = Utc::now();
    record[18] = (now.year() - 1900) as u8;
    record[19] = now.month() as u8;
    record[20] = now.day() as u8;
    record[21] = now.hour() as u8;
    record[22] = now.minute() as u8;
    record[23] = now.second() as u8;
    record[24] = 0;

    record[25] = if is_dir { 0x02 } else { 0x00 };
    record[26] = 0;
    record[27] = 0;
    record[28..30].copy_from_slice(&1u16.to_le_bytes());
    record[30..32].copy_from_slice(&1u16.to_be_bytes());
    record[32] = file_id.len() as u8;
    record[33..33 + file_id.len()].copy_from_slice(file_id);

    record
}

fn pack_records_to_sectors(records: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    for record in records {
        let used_in_sector = out.len() % SECTOR_SIZE;
        let remaining = SECTOR_SIZE - used_in_sector;
        if record.len() > remaining {
            out.resize(out.len() + remaining, 0);
        }
        out.extend_from_slice(&record);
    }

    let pad = (SECTOR_SIZE - (out.len() % SECTOR_SIZE)) % SECTOR_SIZE;
    if pad > 0 {
        out.resize(out.len() + pad, 0);
    }
    out
}

fn write_zeroed_sectors(file: &mut File, start_lba: u32, sector_count: u32) -> std::io::Result<()> {
    let zero = [0u8; SECTOR_SIZE];
    for offset in 0..sector_count {
        write_sector_at(file, start_lba + offset, &zero)?;
    }
    Ok(())
}

fn write_sector_at(file: &mut File, lba: u32, data: &[u8]) -> std::io::Result<()> {
    if data.len() != SECTOR_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "sector write requires 2048-byte data",
        ));
    }
    file.seek(SeekFrom::Start(lba as u64 * SECTOR_SIZE as u64))?;
    file.write_all(data)
}

fn write_at_lba(file: &mut File, lba: u32, data: &[u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(lba as u64 * SECTOR_SIZE as u64))?;
    file.write_all(data)?;

    let pad = (SECTOR_SIZE - (data.len() % SECTOR_SIZE)) % SECTOR_SIZE;
    if pad > 0 {
        let zeros = vec![0u8; pad];
        file.write_all(&zeros)?;
    }

    Ok(())
}

fn write_ascii_padded(dst: &mut [u8], input: &str) {
    dst.fill(b' ');
    let bytes = input.as_bytes();
    let len = bytes.len().min(dst.len());
    dst[..len].copy_from_slice(&bytes[..len]);
}

fn write_u16_both_endian(dst: &mut [u8], value: u16) {
    dst[..2].copy_from_slice(&value.to_le_bytes());
    dst[2..4].copy_from_slice(&value.to_be_bytes());
}

fn write_u32_both_endian(dst: &mut [u8], value: u32) {
    dst[..4].copy_from_slice(&value.to_le_bytes());
    dst[4..8].copy_from_slice(&value.to_be_bytes());
}

fn write_volume_datetime(dst: &mut [u8], ts: chrono::DateTime<Utc>) {
    let mut text = format!(
        "{:04}{:02}{:02}{:02}{:02}{:02}00",
        ts.year(),
        ts.month(),
        ts.day(),
        ts.hour(),
        ts.minute(),
        ts.second()
    )
    .into_bytes();
    text.push(0);
    dst.copy_from_slice(&text[..17]);
}

fn div_ceil(value: usize, divisor: usize) -> usize {
    if value == 0 {
        0
    } else {
        1 + ((value - 1) / divisor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn writes_valid_primary_descriptor_header() {
        let mut output = std::env::temp_dir();
        output.push(format!(
            "bento-cidata-{}.iso",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));

        let files = vec![
            CidataEntry {
                name: "user-data".to_string(),
                contents: b"#cloud-config\n".to_vec(),
            },
            CidataEntry {
                name: "meta-data".to_string(),
                contents: b"instance-id: test\n".to_vec(),
            },
        ];

        write_cidata_iso(&output, "CIDATA", &files).expect("write iso");
        let data = fs::read(&output).expect("read iso");
        let pvd = &data[(SYSTEM_AREA_SECTORS as usize * SECTOR_SIZE)
            ..((SYSTEM_AREA_SECTORS as usize + 1) * SECTOR_SIZE)];

        assert_eq!(pvd[0], 1);
        assert_eq!(&pvd[1..6], b"CD001");
        assert_eq!(&pvd[40..46], b"CIDATA");

        let root_dir = &data[((SYSTEM_AREA_SECTORS as usize + 4) * SECTOR_SIZE)
            ..((SYSTEM_AREA_SECTORS as usize + 5) * SECTOR_SIZE)];
        assert!(root_dir
            .windows("USER-DATA;1".len())
            .any(|w| w == b"USER-DATA;1"));
        assert!(root_dir
            .windows("META-DATA;1".len())
            .any(|w| w == b"META-DATA;1"));

        fs::remove_file(&output).expect("cleanup iso");
    }
}
