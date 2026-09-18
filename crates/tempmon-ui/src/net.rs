//! 网卡信息采样：GetIfTable2 直读接口字节计数（64 位，免驱动、免管理员），
//! 速率由 UI 进程按相邻两次采样的差分计算，无需动 sensor 子进程与共享帧协议。

#![allow(non_snake_case)]

pub struct NetIf {
    pub luid: u64,
    pub alias: String,
    pub connected: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 枚举非回环网卡：已连接的排前面，菜单直接可用
pub fn interfaces() -> Vec<NetIf> {
    use windows::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2, IF_TYPE_SOFTWARE_LOOPBACK,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return Vec::new();
        }
        let n = (*table).NumEntries as usize;
        let base = std::ptr::addr_of!((*table).Table) as *const MIB_IF_ROW2;
        let mut out: Vec<NetIf> = (0..n)
            .map(|i| &*base.add(i))
            .filter(|r| r.Type != IF_TYPE_SOFTWARE_LOOPBACK)
            .map(|r| NetIf {
                luid: r.InterfaceLuid.Value,
                alias: from_wide(&r.Alias),
                connected: r.OperStatus == IfOperStatusUp,
                rx_bytes: r.InOctets,
                tx_bytes: r.OutOctets,
            })
            .collect();
        FreeMibTable(table as _);
        out.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.alias.cmp(&b.alias)));
        out
    }
}

/// 速率显示格式化。unit：0 自动（≥1MB/s 用 MB/s，否则 KB/s）、1 KB/s、2 MB/s
pub fn fmt_speed(v: f32, unit: u8) -> String {
    let kb = (v / 1024.0).max(0.0);
    let mb = (kb / 1024.0).max(0.0);
    match unit {
        1 => format!("{kb:.0}KB/s"),
        2 => format!("{mb:.1}MB/s"),
        _ => {
            if kb >= 1024.0 {
                format!("{mb:.1}MB/s")
            } else {
                format!("{kb:.0}KB/s")
            }
        }
    }
}

/// 胶囊页紧凑格式：省略 "/s"（胶囊窄，两个方向并排，宽度需与 CPU 页对齐）
pub fn fmt_speed_short(v: f32, unit: u8) -> String {
    let kb = (v / 1024.0).max(0.0);
    let mb = (kb / 1024.0).max(0.0);
    match unit {
        1 => format!("{kb:.0}K"),
        2 => format!("{mb:.1}M"),
        _ => {
            if kb >= 1024.0 {
                format!("{mb:.1}M")
            } else {
                format!("{kb:.0}K")
            }
        }
    }
}
