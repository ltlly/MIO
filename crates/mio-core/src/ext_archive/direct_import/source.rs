use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};

use crate::ext_archive::{HEADER_SIZE, MAGIC};
const COMPATIBILITY: u32 = 0x8052_7974;
const MAX_FORWARD_DEPTH: usize = 32;
const BLOCK_BYTES: usize = 64 * 1024;
const CACHE_BLOCKS: usize = 64;
const MAX_COLLECTION_ITEMS: usize = 1_000_000;
pub(crate) const MAX_IMPORT_RAW_BYTES: usize = crate::http::MAX_BODY + crate::http::MAX_HEAD + 4096;
const MAX_UTF16_UNITS: usize = 32_768;

#[derive(Clone, Debug)]
pub(crate) struct SourceHeader {
    pub schema_floor: u16,
    pub schema_revision: u16,
    pub file_size: u64,
    pub boundary: u64,
    pub project_root: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FieldSlot {
    pub id: u8,
    pub offset: u16,
}

#[derive(Clone, Debug)]
pub(crate) struct RawObject {
    pub address: u64,
    pub fields: Vec<FieldSlot>,
}

struct CachedBlock {
    index: u64,
    used: u64,
    len: usize,
    data: Vec<u8>,
}

pub(crate) struct ExtSource {
    file: File,
    pub header: SourceHeader,
    clock: u64,
    cache: Vec<CachedBlock>,
}

impl ExtSource {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let mut opts = OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(0x0000_0007);
        }
        let file = opts.open(path).context("不能只读打开 External 源文件")?;
        let meta = file.metadata().context("不能读取 External 源文件元数据")?;
        ensure!(meta.is_file(), "External 源必须是普通文件");
        let file_size = meta.len();
        ensure!(file_size >= HEADER_SIZE, "外部归档容器头已截断");
        let mut header_bytes = [0u8; HEADER_SIZE as usize];
        read_file_exact_at(&file, 0, &mut header_bytes).context("读取 外部归档容器头失败")?;
        ensure!(
            header_bytes[..4] == MAGIC,
            "文件缺少有效的 外部归档容器魔数"
        );
        let outer_version = i32::from_be_bytes(header_bytes[4..8].try_into().unwrap());
        ensure!(
            outer_version == 1,
            "不支持的 External 外层存储版本 {outer_version}"
        );
        let compatibility = u32::from_be_bytes(header_bytes[8..12].try_into().unwrap());
        ensure!(
            compatibility == COMPATIBILITY,
            "不支持的 外部归档容器兼容标识 {compatibility:#010x}"
        );
        let schema_floor = u16::from_be_bytes(header_bytes[12..14].try_into().unwrap());
        let schema_revision = u16::from_be_bytes(header_bytes[14..16].try_into().unwrap());
        let span = i64::from_be_bytes(header_bytes[48..56].try_into().unwrap());
        ensure!(span == i64::MAX, "不支持多文件分片 外部归档项目");
        let allocation_raw = i64::from_be_bytes(header_bytes[56..64].try_into().unwrap());
        let allocation_cursor =
            u64::try_from(allocation_raw).context("External 分配游标为负数")?;
        ensure!(
            allocation_cursor >= HEADER_SIZE && allocation_cursor <= file_size,
            "External 分配游标超出物理文件范围"
        );
        let root_raw = i64::from_be_bytes(header_bytes[64..72].try_into().unwrap());
        let project_root = u64::try_from(root_raw).context("外部归档项目根地址为负数")?;
        ensure!(
            project_root == 0 || (project_root >= HEADER_SIZE && project_root < allocation_cursor),
            "外部归档项目根地址超出有效范围"
        );
        Ok(Self {
            file,
            header: SourceHeader {
                schema_floor,
                schema_revision,
                file_size,
                boundary: allocation_cursor,
                project_root,
            },
            clock: 0,
            cache: Vec::with_capacity(CACHE_BLOCKS),
        })
    }

    pub(crate) fn read_exact(&mut self, mut offset: u64, mut out: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(out.len() as u64)
            .context("External 读取偏移溢出")?;
        ensure!(end <= self.header.boundary, "External 读取超出有效边界");
        if out.len() >= BLOCK_BYTES {
            return read_file_exact_at(&self.file, offset, out).context("读取 外部归档数据块失败");
        }
        while !out.is_empty() {
            let block_idx = offset / BLOCK_BYTES as u64;
            let in_block = (offset % BLOCK_BYTES as u64) as usize;
            let slot = self.load_block(block_idx)?;
            let block = &self.cache[slot];
            ensure!(in_block < block.len, "外部归档数据块读取意外截断");
            let take = (block.len - in_block).min(out.len());
            out[..take].copy_from_slice(&block.data[in_block..in_block + take]);
            offset += take as u64;
            out = &mut out[take..];
        }
        Ok(())
    }

    fn load_block(&mut self, index: u64) -> Result<usize> {
        self.clock = self.clock.wrapping_add(1);
        let tick = self.clock;
        if let Some(pos) = self.cache.iter().position(|b| b.index == index) {
            self.cache[pos].used = tick;
            return Ok(pos);
        }
        let block_start = index * BLOCK_BYTES as u64;
        let max_len = (self.header.boundary - block_start).min(BLOCK_BYTES as u64) as usize;
        let slot = if self.cache.len() < CACHE_BLOCKS {
            self.cache.push(CachedBlock {
                index,
                used: tick,
                len: max_len,
                data: vec![0u8; BLOCK_BYTES],
            });
            self.cache.len() - 1
        } else {
            let oldest = self
                .cache
                .iter()
                .enumerate()
                .min_by_key(|(_, b)| b.used)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.cache[oldest].index = index;
            self.cache[oldest].used = tick;
            self.cache[oldest].len = max_len;
            oldest
        };
        read_file_exact_at(
            &self.file,
            block_start,
            &mut self.cache[slot].data[..max_len],
        )
        .context("读取 外部归档缓存页失败")?;
        Ok(slot)
    }

    pub(crate) fn object(&mut self, mut offset: u64) -> Result<RawObject> {
        let mut seen = Vec::new();
        loop {
            ensure!(
                offset >= HEADER_SIZE && offset < self.header.boundary,
                "External 对象地址超出有效范围"
            );
            ensure!(!seen.contains(&offset), "External 转发记录存在环路");
            ensure!(
                seen.len() < MAX_FORWARD_DEPTH,
                "External 转发链超过最大深度"
            );
            seen.push(offset);
            let mut first = [0u8; 1];
            self.read_exact(offset, &mut first)?;
            if first[0] & 1 != 0 {
                let mut target_raw = [0u8; 8];
                self.read_exact(offset + 1, &mut target_raw)?;
                let next = i64::from_be_bytes(target_raw);
                offset = u64::try_from(next).context("External 转发目标地址为负数")?;
                continue;
            }
            let mut head = [0u8; 3];
            self.read_exact(offset + 1, &mut head)?;
            let count = usize::from(head[2]);
            let table_bytes = count * 3;
            let mut table = vec![0u8; table_bytes];
            if table_bytes > 0 {
                self.read_exact(offset + 4, &mut table)?;
            }
            let mut fields = Vec::with_capacity(count);
            let min_offset = 4 + table_bytes;
            let mut prev_offset = 0u16;
            for (idx, chunk) in table.as_chunks::<3>().0.iter().enumerate() {
                let id = chunk[0];
                let rel = u16::from_be_bytes([chunk[1], chunk[2]]);
                if idx == 0 {
                    ensure!(usize::from(rel) >= min_offset, "外部归档字段偏移覆盖描述符表");
                } else {
                    ensure!(rel > prev_offset, "外部归档字段偏移未严格递增");
                }
                ensure!(
                    offset
                        .checked_add(u64::from(rel))
                        .is_some_and(|pos| pos < self.header.boundary),
                    "外部归档字段地址越界"
                );
                prev_offset = rel;
                fields.push(FieldSlot { id, offset: rel });
            }
            return Ok(RawObject {
                address: offset,
                fields,
            });
        }
    }

    pub(crate) fn field_span(&self, obj: &RawObject, id: u8) -> Option<(u64, u64)> {
        let idx = obj.fields.iter().position(|f| f.id == id)?;
        let rel = u64::from(obj.fields[idx].offset);
        let start = obj.address.checked_add(rel)?;
        let end = if let Some(next) = obj.fields.get(idx + 1) {
            obj.address.checked_add(u64::from(next.offset))?
        } else {
            self.header.boundary
        };
        (end >= start && end <= self.header.boundary).then_some((start, end - start))
    }

    fn field_bytes<const N: usize>(&mut self, obj: &RawObject, id: u8) -> Result<Option<[u8; N]>> {
        let Some((start, max_width)) = self.field_span(obj, id) else {
            return Ok(None);
        };
        if max_width < N as u64 {
            return Ok(None);
        }
        let mut buf = [0u8; N];
        self.read_exact(start, &mut buf)?;
        Ok(Some(buf))
    }

    pub(crate) fn opt_u8(&mut self, obj: &RawObject, id: u8) -> Result<Option<u8>> {
        Ok(self.field_bytes::<1>(obj, id)?.map(|[v]| v))
    }

    pub(crate) fn opt_bool(&mut self, obj: &RawObject, id: u8) -> Result<Option<bool>> {
        Ok(match self.opt_u8(obj, id)? {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        })
    }

    pub(crate) fn opt_i32(&mut self, obj: &RawObject, id: u8) -> Result<Option<i32>> {
        Ok(self.field_bytes::<4>(obj, id)?.map(i32::from_be_bytes))
    }

    pub(crate) fn opt_i64(&mut self, obj: &RawObject, id: u8) -> Result<Option<i64>> {
        Ok(self.field_bytes::<8>(obj, id)?.map(i64::from_be_bytes))
    }

    pub(crate) fn opt_ref(&mut self, obj: &RawObject, id: u8) -> Result<Option<Option<u64>>> {
        let Some(raw) = self.opt_i64(obj, id)? else {
            return Ok(None);
        };
        if raw == 0 {
            return Ok(Some(None));
        }
        let Some(addr) = u64::try_from(raw).ok() else {
            return Ok(None);
        };
        if addr >= HEADER_SIZE && addr < self.header.boundary {
            Ok(Some(Some(addr)))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn read_reference_array(&mut self, addr: u64, limit: usize) -> Result<Vec<Option<u64>>> {
        ensure!(
            addr >= HEADER_SIZE && addr + 8 <= self.header.boundary,
            "External 引用数组地址越界"
        );
        let mut head = [0u8; 8];
        self.read_exact(addr, &mut head)?;
        let total_size = u64::try_from(i32::from_be_bytes(head[..4].try_into().unwrap()))
            .context("External 引用数组总长度为负数")?;
        let element_count = u64::try_from(i32::from_be_bytes(head[4..8].try_into().unwrap()))
            .context("External 引用数组元素数为负数")?;
        let payload_bytes = element_count
            .checked_mul(8)
            .context("External 引用数组字节数溢出")?;
        let expected_total = payload_bytes.max(1).checked_add(8).context("External 数组总长度溢出")?;
        ensure!(
            total_size == expected_total
                && addr
                    .checked_add(total_size)
                    .is_some_and(|end| end <= self.header.boundary),
            "External 引用数组头长度不匹配"
        );
        let take = (element_count as usize).min(limit);
        let mut raw = vec![0u8; take * 8];
        if !raw.is_empty() {
            self.read_exact(addr + 8, &mut raw)?;
        }
        let mut out = Vec::with_capacity(take);
        for slot in raw.as_chunks::<8>().0 {
            let val = i64::from_be_bytes(*slot);
            if val == 0 {
                out.push(None);
            } else {
                let target = u64::try_from(val).context("External 引用槽位为负数")?;
                ensure!(
                    target >= HEADER_SIZE && target < self.header.boundary,
                    "External 引用槽位越界"
                );
                out.push(Some(target));
            }
        }
        Ok(out)
    }

    pub(crate) fn read_collection_slots(&mut self, addr: u64) -> Result<Vec<Option<u64>>> {
        let obj = self.object(addr)?;
        let len_raw = self.opt_i32(&obj, 0)?.context("集合缺少 f0 逻辑长度")?;
        let logical_len = usize::try_from(len_raw).context("集合逻辑长度为负数")?;
        ensure!(
            logical_len <= MAX_COLLECTION_ITEMS,
            "集合条目数超过 {MAX_COLLECTION_ITEMS} 上限"
        );
        if logical_len == 0 {
            return Ok(Vec::new());
        }
        let is_chunked = self
            .field_span(&obj, 1)
            .is_some_and(|(_, width)| width == 4)
            && obj.fields.iter().any(|f| f.id == 2);
        if is_chunked {
            let chunk_width_raw = self.opt_i32(&obj, 1)?.context("分块集合缺少块宽度")?;
            let chunk_width = usize::try_from(chunk_width_raw)
                .ok()
                .filter(|w| *w > 0)
                .context("分块集合块宽度无效")?;
            let leading = self
                .opt_i32(&obj, 3)?
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(0);
            let chunks_addr = self
                .opt_ref(&obj, 2)?
                .flatten()
                .context("分块集合缺少块列表引用")?;
            let chunks_obj = self.object(chunks_addr)?;
            let active_chunks = self
                .opt_i32(&chunks_obj, 0)?
                .and_then(|v| usize::try_from(v).ok())
                .context("分块集合活动块数无效")?;
            let refs_addr = self
                .opt_ref(&chunks_obj, 1)?
                .flatten()
                .context("分块集合缺少块引用数组")?;
            let chunk_addrs = self.read_reference_array(refs_addr, active_chunks)?;
            let total_extent = leading
                .checked_add(logical_len)
                .context("分块集合逻辑跨度溢出")?;
            let needed_chunks = total_extent.div_ceil(chunk_width);
            ensure!(
                chunk_addrs.len() >= needed_chunks,
                "分块集合活动块不足以覆盖逻辑长度"
            );
            let mut out = Vec::with_capacity(logical_len);
            for (chunk_idx, &chunk_opt) in chunk_addrs
                .iter()
                .enumerate()
                .take(needed_chunks)
                .skip(leading / chunk_width)
            {
                let start_in_chunk = if chunk_idx == leading / chunk_width {
                    leading % chunk_width
                } else {
                    0
                };
                let remain = logical_len - out.len();
                let take = (chunk_width - start_in_chunk).min(remain);
                if let Some(chunk_addr) = chunk_opt {
                    let slots =
                        self.read_reference_array(chunk_addr, start_in_chunk + take)?;
                    ensure!(
                        slots.len() >= start_in_chunk + take,
                        "分块集合子块容量小于所需槽位数"
                    );
                    out.extend_from_slice(&slots[start_in_chunk..start_in_chunk + take]);
                } else {
                    out.extend(std::iter::repeat_n(None, take));
                }
            }
            Ok(out)
        } else {
            let Some(refs_addr) = self.opt_ref(&obj, 1)?.flatten() else {
                bail!("直接集合缺少背板引用数组");
            };
            let slots = self.read_reference_array(refs_addr, logical_len)?;
            ensure!(slots.len() == logical_len, "直接集合容量小于逻辑长度");
            Ok(slots)
        }
    }

    pub(crate) fn read_bytes(&mut self, addr: u64) -> Result<Option<Vec<u8>>> {
        if addr < HEADER_SIZE || addr + 8 > self.header.boundary {
            return Ok(None);
        }
        let mut head = [0u8; 8];
        self.read_exact(addr, &mut head)?;
        let total_size = i32::from_be_bytes(head[..4].try_into().unwrap());
        let element_count = i32::from_be_bytes(head[4..8].try_into().unwrap());
        if total_size >= 9
            && element_count >= 0
            && (total_size as u64) == (element_count as u64).max(1) + 8
            && addr
                .checked_add(total_size as u64)
                .is_some_and(|end| end <= self.header.boundary)
        {
            let take = (element_count as usize).min(MAX_IMPORT_RAW_BYTES);
            let mut out = vec![0u8; take];
            if take > 0 {
                self.read_exact(addr + 8, &mut out)?;
            }
            return Ok(Some(out));
        }

        let obj = self.object(addr)?;
        let Some(logical_raw) = self.opt_i32(&obj, 0)? else {
            return Ok(None);
        };
        if logical_raw < 0 {
            return Ok(None);
        }
        if logical_raw == 0 {
            return Ok(Some(Vec::new()));
        }
        let target_len = (logical_raw as usize).min(MAX_IMPORT_RAW_BYTES);
        let Some(wrapper_addr) = self.opt_ref(&obj, 1)?.flatten() else {
            return Ok(None);
        };
        let wrapper = self.object(wrapper_addr)?;
        let Some(active_raw) = self.opt_i32(&wrapper, 0)? else {
            return Ok(None);
        };
        if active_raw <= 0 {
            return Ok(Some(Vec::new()));
        }
        let Some(refs_addr) = self.opt_ref(&wrapper, 1)?.flatten() else {
            return Ok(None);
        };
        let chunk_addrs = self.read_reference_array(refs_addr, active_raw as usize)?;
        let mut out = Vec::with_capacity(target_len);
        for chunk_addr in chunk_addrs.into_iter().flatten() {
            if out.len() >= target_len {
                break;
            }
            if chunk_addr + 8 > self.header.boundary {
                break;
            }
            let mut chead = [0u8; 8];
            self.read_exact(chunk_addr, &mut chead)?;
            let cts = i32::from_be_bytes(chead[..4].try_into().unwrap());
            let cec = i32::from_be_bytes(chead[4..8].try_into().unwrap());
            if cts < 9 || cec < 0 || (cts as u64) != (cec as u64).max(1) + 8 {
                return Ok(None);
            }
            let take = (cec as usize).min(target_len - out.len());
            if take > 0 {
                let start = out.len();
                out.resize(start + take, 0);
                self.read_exact(chunk_addr + 8, &mut out[start..])?;
            }
        }
        Ok(Some(out))
    }

    fn read_utf16_array_units(&mut self, addr: u64, max_units: usize) -> Result<Option<Vec<u16>>> {
        if addr < HEADER_SIZE || addr + 8 > self.header.boundary {
            return Ok(None);
        }
        let mut head = [0u8; 8];
        self.read_exact(addr, &mut head)?;
        let total_size = i32::from_be_bytes(head[..4].try_into().unwrap());
        let element_count = i32::from_be_bytes(head[4..8].try_into().unwrap());
        if total_size < 9 || element_count < 0 {
            return Ok(None);
        }
        let Some(payload_bytes) = (element_count as u64).checked_mul(2) else {
            return Ok(None);
        };
        if (total_size as u64) != payload_bytes.max(1) + 8 {
            return Ok(None);
        }
        if addr
            .checked_add(total_size as u64)
            .is_none_or(|end| end > self.header.boundary)
        {
            return Ok(None);
        }
        let take = (element_count as usize).min(max_units);
        let mut raw = vec![0u8; take * 2];
        if take > 0 {
            self.read_exact(addr + 8, &mut raw)?;
        }
        Ok(Some(
            raw.as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect(),
        ))
    }

    pub(crate) fn read_utf16_string(
        &mut self,
        addr: u64,
        max_units: usize,
    ) -> Result<Option<String>> {
        let cap = max_units.min(MAX_UTF16_UNITS);
        if let Some(units) = self.read_utf16_array_units(addr, cap)? {
            return Ok(Some(String::from_utf16_lossy(&units)));
        }
        let obj = self.object(addr)?;
        if self.field_span(&obj, 0).is_some_and(|(_, w)| w == 4)
            && obj.fields.iter().any(|f| f.id == 3)
        {
            let Some(len_raw) = self.opt_i32(&obj, 0)? else {
                return Ok(None);
            };
            if len_raw <= 0 {
                return Ok(Some(String::new()));
            }
            let target = (len_raw as usize).min(cap);
            let Some(wrapper_addr) = self.opt_ref(&obj, 3)?.flatten() else {
                return Ok(None);
            };
            let wrapper = self.object(wrapper_addr)?;
            let Some(active) = self.opt_i32(&wrapper, 0)? else {
                return Ok(None);
            };
            if active <= 0 {
                return Ok(Some(String::new()));
            }
            let Some(refs_addr) = self.opt_ref(&wrapper, 1)?.flatten() else {
                return Ok(None);
            };
            let chunks = self.read_reference_array(refs_addr, active as usize)?;
            let mut units = Vec::with_capacity(target);
            for chunk_addr in chunks.into_iter().flatten() {
                if units.len() >= target {
                    break;
                }
                let remain = target - units.len();
                let Some(part) = self.read_utf16_array_units(chunk_addr, remain)? else {
                    return Ok(None);
                };
                units.extend(part);
            }
            return Ok(Some(String::from_utf16_lossy(&units)));
        }
        if obj.fields.iter().any(|f| f.id == 1)
            && self.field_span(&obj, 2).is_some_and(|(_, w)| w >= 4)
            && let Some(len_raw) = self.opt_i32(&obj, 2)?
        {
            if len_raw <= 0 {
                return Ok(Some(String::new()));
            }
            let Some(backing) = self.opt_ref(&obj, 1)?.flatten() else {
                return Ok(None);
            };
            let Some(units) =
                self.read_utf16_array_units(backing, (len_raw as usize).min(cap))?
            else {
                return Ok(None);
            };
            return Ok(Some(String::from_utf16_lossy(&units)));
        }
        if let Some(arr_opt) = self.opt_ref(&obj, 0)? {
            let Some(arr_addr) = arr_opt else {
                return Ok(Some(String::new()));
            };
            let Some(units) = self.read_utf16_array_units(arr_addr, cap)? else {
                return Ok(None);
            };
            return Ok(Some(String::from_utf16_lossy(&units)));
        }
        Ok(None)
    }

    pub(crate) fn read_service_origin(&mut self, addr: u64) -> Result<Option<String>> {
        let service = self.object(addr)?;
        let Some(host_addr) = self.opt_ref(&service, 0)?.flatten() else {
            return Ok(None);
        };
        let Some(host) = self.read_utf16_string(host_addr, 4096)? else {
            return Ok(None);
        };
        let host = host.trim();
        if host.is_empty() {
            return Ok(None);
        }
        let Some(port_raw) = self.opt_i32(&service, 1)? else {
            return Ok(None);
        };
        let Some(port) = u16::try_from(port_raw).ok().filter(|p| *p > 0) else {
            return Ok(None);
        };
        let tls = self.opt_bool(&service, 2)?.unwrap_or(port == 443);
        let scheme = if tls { "https" } else { "http" };
        let authority = if host.contains(':') && !host.starts_with('[') {
            if host.parse::<std::net::Ipv6Addr>().is_err() {
                return Ok(None);
            }
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        let origin = format!("{scheme}://{authority}:{port}");
        if crate::validate_target_origin(&origin).is_err() {
            return Ok(None);
        }
        Ok(Some(origin))
    }
}

fn read_file_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut offset = offset;
        let mut buf = buf;
        while !buf.is_empty() {
            let n = file.seek_read(buf, offset)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading External source",
                ));
            }
            offset += n as u64;
            buf = &mut buf[n..];
        }
        Ok(())
    }
}
