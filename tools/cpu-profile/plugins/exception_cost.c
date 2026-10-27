/* Count guest instructions executed with a selected Cortex-M exception active. */
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "qemu-plugin.h"

QEMU_PLUGIN_EXPORT int qemu_plugin_version = QEMU_PLUGIN_VERSION;

#define MAX_VCPUS 64
#define TARGET_VCPU 0
#define TARGET_EXCEPTION 31 /* External IRQ 15 on the RT1041 Cortex-M7. */

typedef struct {
    struct qemu_plugin_register *xpsr;
    GByteArray *value;
} VcpuState;

static VcpuState vcpus[MAX_VCPUS];
static uint64_t exception_instructions;
static uint64_t total_instructions;
static uint64_t unclassified_blocks;

static void vcpu_init(unsigned int vcpu_index, void *userdata)
{
    (void)userdata;
    if (vcpu_index >= MAX_VCPUS) {
        return;
    }

    g_autoptr(GArray) registers = qemu_plugin_get_registers();
    for (guint i = 0; i < registers->len; i++) {
        qemu_plugin_reg_descriptor *reg =
            &g_array_index(registers, qemu_plugin_reg_descriptor, i);
        if (g_ascii_strcasecmp(reg->name, "xpsr") == 0) {
            vcpus[vcpu_index].xpsr = reg->handle;
            vcpus[vcpu_index].value = g_byte_array_sized_new(4);
            return;
        }
    }
    fprintf(stderr, "l6-exception-cost: vcpu=%u has no XPSR register\n",
            vcpu_index);
}

static void count_block(unsigned int vcpu_index, void *userdata)
{
    if (vcpu_index != TARGET_VCPU || vcpu_index >= MAX_VCPUS) {
        return;
    }

    uint64_t instructions = (uintptr_t)userdata;
    __atomic_fetch_add(&total_instructions, instructions, __ATOMIC_RELAXED);
    if (!vcpus[vcpu_index].xpsr) {
        __atomic_fetch_add(&unclassified_blocks, 1, __ATOMIC_RELAXED);
        return;
    }

    GByteArray *value = vcpus[vcpu_index].value;
    g_byte_array_set_size(value, 0);
    if (!qemu_plugin_read_register(vcpus[vcpu_index].xpsr, value) ||
        value->len != sizeof(uint32_t)) {
        __atomic_fetch_add(&unclassified_blocks, 1, __ATOMIC_RELAXED);
        return;
    }

    uint32_t xpsr;
    memcpy(&xpsr, value->data, sizeof(xpsr));
    if ((xpsr & 0x1ffu) == TARGET_EXCEPTION) {
        __atomic_fetch_add(&exception_instructions, instructions,
                           __ATOMIC_RELAXED);
    }
}

static void translate_block(struct qemu_plugin_tb *tb, void *userdata)
{
    (void)userdata;
    size_t instruction_count = qemu_plugin_tb_n_insns(tb);
    qemu_plugin_register_vcpu_tb_exec_cb(
        tb, count_block, QEMU_PLUGIN_CB_R_REGS,
        (void *)(uintptr_t)instruction_count);
}

static void report_counts(void *userdata)
{
    (void)userdata;
    fprintf(stderr,
            "l6-exception-cost: vcpu=%u exception=%u instructions=%" PRIu64
            " total_instructions=%" PRIu64 " unclassified_blocks=%" PRIu64 "\n",
            TARGET_VCPU, TARGET_EXCEPTION,
            __atomic_load_n(&exception_instructions, __ATOMIC_RELAXED),
            __atomic_load_n(&total_instructions, __ATOMIC_RELAXED),
            __atomic_load_n(&unclassified_blocks, __ATOMIC_RELAXED));
    for (unsigned int i = 0; i < MAX_VCPUS; i++) {
        if (vcpus[i].value) {
            g_byte_array_unref(vcpus[i].value);
        }
    }
}

QEMU_PLUGIN_EXPORT int qemu_plugin_install(qemu_plugin_id_t id,
                                           const qemu_info_t *info,
                                           int argc, char **argv)
{
    (void)argc;
    (void)argv;
    if (!info->system_emulation || info->system.max_vcpus > MAX_VCPUS) {
        fprintf(stderr, "l6-exception-cost: unsupported QEMU configuration\n");
        return -1;
    }

    qemu_plugin_register_vcpu_init_cb(id, vcpu_init, NULL);
    qemu_plugin_register_vcpu_tb_trans_cb(id, translate_block, NULL);
    qemu_plugin_register_atexit_cb(id, report_counts, NULL);
    return 0;
}
