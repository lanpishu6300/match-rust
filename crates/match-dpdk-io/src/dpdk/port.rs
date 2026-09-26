//! Port configuration helper: a single rx/tx queue on `port` backed by the
//! shared mbuf pool. Shared by the rx/tx verification bins.

use super::*;

/// Configure + start a port with one rx and one tx queue.
pub unsafe fn setup_port(port: u16, pool: *mut rte_mempool) -> Result<(), String> {
    let mut conf: rte_eth_conf = std::mem::zeroed();
    let rc = rte_eth_dev_configure(port, 1, 1, &mut conf as *mut rte_eth_conf);
    if rc != 0 {
        return Err(format!("rte_eth_dev_configure({port}) failed: {rc}"));
    }

    let mut rxconf: rte_eth_rxconf = std::mem::zeroed();
    let rc = rte_eth_rx_queue_setup(port, 0, 256, rte_eth_dev_socket_id(port) as u32, &mut rxconf as *mut rte_eth_rxconf, pool);
    if rc != 0 {
        return Err(format!("rte_eth_rx_queue_setup({port}) failed: {rc}"));
    }

    let mut txconf: rte_eth_txconf = std::mem::zeroed();
    let rc = rte_eth_tx_queue_setup(port, 0, 256, rte_eth_dev_socket_id(port) as u32, &mut txconf as *mut rte_eth_txconf);
    if rc != 0 {
        return Err(format!("rte_eth_tx_queue_setup({port}) failed: {rc}"));
    }

    let rc = rte_eth_dev_start(port);
    if rc != 0 {
        return Err(format!("rte_eth_dev_start({port}) failed: {rc}"));
    }
    Ok(())
}

/// One rx burst pass: pull up to `BURST_SIZE` mbufs and return them.
pub unsafe fn rx_burst(port: u16, mbufs: &mut [*mut rte_mbuf]) -> u16 {
    rte_eth_rx_burst(port, 0, mbufs.as_mut_ptr(), mbufs.len() as u16)
}

/// 在 mbuf data room 内直接构建帧（零中间 Vec）：`build` 写入可写区并返回实际长度，
/// 结束后原地收窄 `pkt_len`/`data_len` 到实际长度。相比 `tx_frame`（组装 Vec + memcpy）
/// 省去整帧拷贝，适合下单回报/行情等热路径。
pub unsafe fn tx_frame_from(
    port: u16,
    pool: *mut rte_mempool,
    capacity: u16,
    build: impl FnOnce(&mut [u8]) -> usize,
) -> Result<(), String> {
    let mbuf = rte_pktmbuf_alloc(pool);
    if mbuf.is_null() {
        return Err("rte_pktmbuf_alloc failed".into());
    }
    let dst = rte_pktmbuf_append(mbuf, capacity);
    if dst.is_null() {
        rte_pktmbuf_free(mbuf);
        return Err("rte_pktmbuf_append failed".into());
    }
    let buf = std::slice::from_raw_parts_mut(dst, capacity as usize);
    let used = build(buf);
    if used == 0 || used > capacity as usize {
        rte_pktmbuf_free(mbuf);
        return Err(format!("tx_frame_from build returned {used}/{capacity}"));
    }
    if used as u16 != capacity {
        // 原地收窄帧长（等效 rte_pktmbuf_trim，本项目 FFI 直接维护这两个字段）
        (*mbuf).pkt_len = used as u32;
        (*mbuf).data_len = used as u16;
    }
    let mut txd: [*mut rte_mbuf; 1] = [mbuf];
    let sent = rte_eth_tx_burst(port, 0, txd.as_mut_ptr(), 1);
    if sent != 1 {
        rte_pktmbuf_free(mbuf);
        return Err(format!("rte_eth_tx_burst sent {sent}/1"));
    }
    Ok(())
}

/// Send a raw Ethernet frame (built by `frames::eth_ip_udp_header` + payload)
/// through DPDK tx. Returns Ok(()) when the frame was queued.
pub unsafe fn tx_frame(port: u16, pool: *mut rte_mempool, frame: &[u8]) -> Result<(), String> {
    let mbuf = rte_pktmbuf_alloc(pool);
    if mbuf.is_null() {
        return Err("rte_pktmbuf_alloc failed".into());
    }
    // rte_pktmbuf_append returns the data pointer to memcpy into.
    let dst = rte_pktmbuf_append(mbuf, frame.len() as u16);
    if dst.is_null() {
        rte_pktmbuf_free(mbuf);
        return Err("rte_pktmbuf_append failed".into());
    }
    std::ptr::copy_nonoverlapping(frame.as_ptr(), dst, frame.len());
    let mut txd: [*mut rte_mbuf; 1] = [mbuf];
    let sent = rte_eth_tx_burst(port, 0, txd.as_mut_ptr(), 1);
    if sent != 1 {
        rte_pktmbuf_free(mbuf);
        return Err(format!("rte_eth_tx_burst sent {sent}/1"));
    }
    Ok(())
}
