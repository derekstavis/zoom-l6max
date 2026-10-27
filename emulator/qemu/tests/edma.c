#include "../hw/arm/l6-edma.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

typedef struct TestMemory {
    uint8_t bytes[256];
    uint8_t sink[32];
    unsigned sink_size;
} TestMemory;

static bool test_read(void *opaque, uint32_t address, void *buffer, size_t size)
{
    TestMemory *memory = opaque;
    if (address + size > sizeof(memory->bytes)) {
        return false;
    }
    memcpy(buffer, memory->bytes + address, size);
    return true;
}

static bool test_write(void *opaque, uint32_t address, const void *buffer,
                       size_t size)
{
    TestMemory *memory = opaque;
    if (address == 0x1000 && memory->sink_size + size <= sizeof(memory->sink)) {
        memcpy(memory->sink + memory->sink_size, buffer, size);
        memory->sink_size += size;
        return true;
    }
    if (address + size > sizeof(memory->bytes)) {
        return false;
    }
    memcpy(memory->bytes + address, buffer, size);
    return true;
}

int main(void)
{
    uint8_t raw[L6_EDMA_TCD_BYTES] = { 0 };
    L6EdmaTcd tcd = {
        .saddr = 0x20,
        .soff = 4,
        .attr = 0x0202, /* 32-bit source and destination elements. */
        .nbytes = 8,
        .daddr = 0x1000,
        .doff = 0,
        .citer = 4,
        /* RT1041 CSR encoding observed on L6 channel 15: ESG, INTHALF,
         * and INTMAJOR are bits 4, 2, and 1 (0x0016). */
        .csr = 0x0016,
        .biter = 4,
    };
    TestMemory memory = { 0 };

    for (unsigned i = 0; i < 32; i++) {
        memory.bytes[0x20 + i] = i;
    }
    l6_edma_tcd_encode(raw, &tcd);
    assert(L6_EDMA_TCD_ESG == 0x0010);
    assert(L6_EDMA_TCD_INTHALF == 0x0004);
    assert(L6_EDMA_TCD_INTMAJOR == 0x0002);
    L6EdmaTcd decoded;
    l6_edma_tcd_decode(raw, &decoded);
    assert(decoded.saddr == tcd.saddr && decoded.daddr == tcd.daddr);
    assert(decoded.nbytes == 8 && decoded.citer == 4 && decoded.biter == 4);

    assert(l6_edma_transfer_minor(&decoded, test_read, test_write, &memory) ==
           L6_EDMA_MINOR_MORE);
    assert(decoded.saddr == 0x28 && decoded.citer == 3);
    assert(l6_edma_transfer_minor(&decoded, test_read, test_write, &memory) ==
           L6_EDMA_MINOR_HALF);
    assert(decoded.saddr == 0x30 && decoded.citer == 2);
    assert(l6_edma_transfer_minor(&decoded, test_read, test_write, &memory) ==
           L6_EDMA_MINOR_MORE);
    assert(l6_edma_transfer_minor(&decoded, test_read, test_write, &memory) ==
           L6_EDMA_MINOR_MAJOR);
    assert(decoded.saddr == 0x40 && decoded.citer == 0);
    assert(memory.sink_size == 32);
    for (unsigned i = 0; i < memory.sink_size; i++) {
        assert(memory.sink[i] == i);
    }

    /* The RT eDMA reloads CITER and applies the final address offsets after
     * a non-ESG major loop. DREQ alone requests ERQ disable. */
    decoded = (L6EdmaTcd) {
        .saddr = 0x80,
        .slast = -16,
        .daddr = 0x1000,
        .dlast_sga = -8,
        .citer = 0,
        .biter = 4,
        .csr = 0,
    };
    assert(!l6_edma_complete_major(&decoded));
    assert(decoded.saddr == 0x70 && decoded.daddr == 0xff8);
    assert(decoded.citer == 4 && (decoded.csr & L6_EDMA_TCD_DONE));
    decoded.csr |= L6_EDMA_TCD_DREQ;
    assert(l6_edma_complete_major(&decoded));

    decoded.citer = 1;
    decoded.nbytes = 6;
    assert(l6_edma_transfer_minor(&decoded, test_read, test_write, &memory) ==
           L6_EDMA_MINOR_ERROR);

    puts("RT eDMA TCD/minor-loop checks passed");
    return 0;
}
