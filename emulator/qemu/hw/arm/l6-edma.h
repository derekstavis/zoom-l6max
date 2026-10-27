/* SPDX-License-Identifier: GPL-2.0-or-later */
/* RT1041 32-bit eDMA TCD decode and one-request minor-loop executor. */
#ifndef HW_ARM_L6_EDMA_H
#define HW_ARM_L6_EDMA_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define L6_EDMA_TCD_BYTES 32
#define L6_EDMA_TCD_ESG (1u << 4)
#define L6_EDMA_TCD_DREQ (1u << 3)
#define L6_EDMA_TCD_INTHALF (1u << 2)
#define L6_EDMA_TCD_INTMAJOR (1u << 1)
#define L6_EDMA_TCD_DONE (1u << 7)

typedef struct L6EdmaTcd {
    uint32_t saddr;
    int16_t soff;
    uint16_t attr;
    uint32_t nbytes;
    int32_t slast;
    uint32_t daddr;
    int16_t doff;
    uint16_t citer;
    int32_t dlast_sga;
    uint16_t csr;
    uint16_t biter;
} L6EdmaTcd;

typedef bool (*L6EdmaRead)(void *opaque, uint32_t address, void *buffer,
                           size_t size);
typedef bool (*L6EdmaWrite)(void *opaque, uint32_t address,
                            const void *buffer, size_t size);

typedef enum L6EdmaMinorResult {
    L6_EDMA_MINOR_ERROR = -1,
    L6_EDMA_MINOR_MORE = 0,
    L6_EDMA_MINOR_HALF = 1,
    L6_EDMA_MINOR_MAJOR = 2,
} L6EdmaMinorResult;

/* Apply the RT eDMA major-loop side effects for a non-ESG descriptor. Return
 * whether CSR.DREQ requests that the controller disable this channel's ERQ. */
static inline bool l6_edma_complete_major(L6EdmaTcd *tcd)
{
    tcd->saddr += tcd->slast;
    tcd->daddr += tcd->dlast_sga;
    tcd->citer = tcd->biter;
    tcd->csr |= L6_EDMA_TCD_DONE;
    return (tcd->csr & L6_EDMA_TCD_DREQ) != 0;
}

static inline uint16_t l6_edma_load16(const uint8_t *p)
{
    return (uint16_t)p[0] | (uint16_t)p[1] << 8;
}

static inline uint32_t l6_edma_load32(const uint8_t *p)
{
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 |
           (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

static inline void l6_edma_store16(uint8_t *p, uint16_t value)
{
    p[0] = value;
    p[1] = value >> 8;
}

static inline void l6_edma_store32(uint8_t *p, uint32_t value)
{
    p[0] = value;
    p[1] = value >> 8;
    p[2] = value >> 16;
    p[3] = value >> 24;
}

static inline void l6_edma_tcd_decode(const uint8_t bytes[L6_EDMA_TCD_BYTES],
                                     L6EdmaTcd *tcd)
{
    tcd->saddr = l6_edma_load32(bytes + 0);
    tcd->soff = (int16_t)l6_edma_load16(bytes + 4);
    tcd->attr = l6_edma_load16(bytes + 6);
    tcd->nbytes = l6_edma_load32(bytes + 8);
    tcd->slast = (int32_t)l6_edma_load32(bytes + 12);
    tcd->daddr = l6_edma_load32(bytes + 16);
    tcd->doff = (int16_t)l6_edma_load16(bytes + 20);
    tcd->citer = l6_edma_load16(bytes + 22);
    tcd->dlast_sga = (int32_t)l6_edma_load32(bytes + 24);
    tcd->csr = l6_edma_load16(bytes + 28);
    tcd->biter = l6_edma_load16(bytes + 30);
}

static inline void l6_edma_tcd_encode(uint8_t bytes[L6_EDMA_TCD_BYTES],
                                     const L6EdmaTcd *tcd)
{
    l6_edma_store32(bytes + 0, tcd->saddr);
    l6_edma_store16(bytes + 4, (uint16_t)tcd->soff);
    l6_edma_store16(bytes + 6, tcd->attr);
    l6_edma_store32(bytes + 8, tcd->nbytes);
    l6_edma_store32(bytes + 12, (uint32_t)tcd->slast);
    l6_edma_store32(bytes + 16, tcd->daddr);
    l6_edma_store16(bytes + 20, (uint16_t)tcd->doff);
    l6_edma_store16(bytes + 22, tcd->citer);
    l6_edma_store32(bytes + 24, (uint32_t)tcd->dlast_sga);
    l6_edma_store16(bytes + 28, tcd->csr);
    l6_edma_store16(bytes + 30, tcd->biter);
}

/* Execute exactly one peripheral request: one NBYTES minor loop. This core
 * intentionally supports the RT1041's observed equal-width, no-MLOFF TCDs;
 * scatter/gather reload and channel IRQ state are handled by the controller. */
static inline L6EdmaMinorResult
l6_edma_transfer_minor(L6EdmaTcd *tcd, L6EdmaRead read_fn,
                       L6EdmaWrite write_fn, void *opaque)
{
    unsigned source_size = 1u << ((tcd->attr >> 8) & 7);
    unsigned dest_size = 1u << (tcd->attr & 7);
    uint8_t bytes[4];

    if ((tcd->citer & 0x8000) || (tcd->biter & 0x8000) ||
        !tcd->citer || !tcd->nbytes || source_size != dest_size ||
        source_size > sizeof(bytes) || tcd->nbytes % source_size) {
        return L6_EDMA_MINOR_ERROR;
    }

    for (uint32_t done = 0; done < tcd->nbytes; done += source_size) {
        if (!read_fn(opaque, tcd->saddr, bytes, source_size) ||
            !write_fn(opaque, tcd->daddr, bytes, source_size)) {
            return L6_EDMA_MINOR_ERROR;
        }
        tcd->saddr += tcd->soff;
        tcd->daddr += tcd->doff;
    }

    tcd->citer--;
    if ((tcd->csr & L6_EDMA_TCD_INTHALF) && tcd->biter > 1 &&
        tcd->citer == tcd->biter / 2) {
        return L6_EDMA_MINOR_HALF;
    }
    return tcd->citer ? L6_EDMA_MINOR_MORE : L6_EDMA_MINOR_MAJOR;
}

#endif /* HW_ARM_L6_EDMA_H */
