/*
 * DPDK 23.11 inline-API bridge.
 *
 * Ubuntu noble's librte_* .so.24.0 no longer exports the data-plane entry
 * points (rte_eth_rx_burst / rte_eth_tx_burst / rte_pktmbuf_alloc / append /
 * free are all header inline functions in 23.11). The Rust side links
 * against this tiny C shim, which is compiled against the DPDK headers
 * shipped by libdpdk-dev so the inline implementations are used verbatim.
 */
#include <rte_ethdev.h>
#include <rte_mbuf.h>
#include <rte_mempool.h>

uint16_t bridge_rx_burst(uint16_t port_id, uint16_t queue_id,
                         struct rte_mbuf **rx_pkts, uint16_t nb_pkts) {
    return rte_eth_rx_burst(port_id, queue_id, rx_pkts, nb_pkts);
}

uint16_t bridge_tx_burst(uint16_t port_id, uint16_t queue_id,
                         struct rte_mbuf **tx_pkts, uint16_t nb_pkts) {
    return rte_eth_tx_burst(port_id, queue_id, tx_pkts, nb_pkts);
}

struct rte_mbuf *bridge_pktmbuf_alloc(struct rte_mempool *mp) {
    return rte_pktmbuf_alloc(mp);
}

void *bridge_pktmbuf_append(struct rte_mbuf *m, uint16_t len) {
    return rte_pktmbuf_append(m, len);
}

void bridge_pktmbuf_free(struct rte_mbuf *m) {
    rte_pktmbuf_free(m);
}
