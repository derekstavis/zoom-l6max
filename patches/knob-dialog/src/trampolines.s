// Displaced instructions are loaded only from locally supplied firmware.
// The patcher verifies the full source hash and each hook region first.
.thumb
.global original_notification_pop
.thumb_func
original_notification_pop:
.incbin "main_firmware.bin", 0x6110, 4
ldr pc, =0x80006115
.ltorg
.global original_notification_tick
.thumb_func
original_notification_tick:
.incbin "main_firmware.bin", 0x64c0, 6
ldr pc, =0x800064c7
.ltorg
.global original_notification_show
.thumb_func
original_notification_show:
.incbin "main_firmware.bin", 0x6660, 4
ldr pc, =0x80006665
.ltorg
.global original_overlays_clear
.thumb_func
original_overlays_clear:
.incbin "main_firmware.bin", 0x63b8, 6
ldr pc, =0x800063bf
.ltorg
.global original_busy_hide
.thumb_func
original_busy_hide:
.incbin "main_firmware.bin", 0x6370, 6
ldr pc, =0x80006377
.ltorg
.global original_busy_show
.thumb_func
original_busy_show:
.incbin "main_firmware.bin", 0x68a0, 4
ldr pc, =0x800068a5
.ltorg
.global original_notification_pause
.thumb_func
original_notification_pause:
cbz r0, 1f
.incbin "main_firmware.bin", 0x62d2, 2
ldr pc, =0x800062d5
1: ldr pc, =0x80006325
.ltorg
.global original_notification_remove
.thumb_func
original_notification_remove:
.incbin "main_firmware.bin", 0x6070, 4
ldr pc, =0x80006075
.ltorg
.global original_notification_current
.thumb_func
original_notification_current:
.incbin "main_firmware.bin", 0x6348, 4
ldr pc, =0x8000634d
.ltorg
.global original_notification_redraw_request
.thumb_func
original_notification_redraw_request:
.incbin "main_firmware.bin", 0x65f8, 4
ldr pc, =0x800065fd
.ltorg
.global original_busy_tick
.thumb_func
original_busy_tick:
.incbin "main_firmware.bin", 0x6560, 6
ldr pc, =0x80006567
.ltorg
.global original_notification_draw
.thumb_func
original_notification_draw:
push {r4,r5,r6,lr}
ldr r12, =0x8000afd1
blx r12
ldr pc, =0x800068ff
.ltorg
.global original_notification_bind
.thumb_func
original_notification_bind:
.incbin "main_firmware.bin", 0x69b0, 6
ldr pc, =0x800069b7
.ltorg
.global original_display_scene_refresh
.thumb_func
original_display_scene_refresh:
.incbin "main_firmware.bin", 0x6bd0, 4
ldr pc, =0x80006bd5
.ltorg
.global original_widget_polarity
.thumb_func
original_widget_polarity:
.incbin "main_firmware.bin", 0x7080, 4
ldr pc, =0x80007085
.ltorg
.global original_widget_y
.thumb_func
original_widget_y:
.incbin "main_firmware.bin", 0x70b0, 4
ldr pc, =0x800070b5
.ltorg
.global original_widget_resource
.thumb_func
original_widget_resource:
.incbin "main_firmware.bin", 0x7090, 4
ldr pc, =0x80007095
.ltorg
.global original_localized_text
.thumb_func
original_localized_text:
.incbin "main_firmware.bin", 0x6fa8, 4
ldr pc, =0x80006fad
.ltorg
.global original_text_layout
.thumb_func
original_text_layout:
.incbin "main_firmware.bin", 0x61ab0, 4
ldr pc, =0x80061ab5
.ltorg
.global original_text_measure
.thumb_func
original_text_measure:
.incbin "main_firmware.bin", 0x6d50, 4
ldr pc, =0x80006d55
.ltorg
.global original_text_copy
.thumb_func
original_text_copy:
cbz r0, 1f
movw r12, #0x50fc
ldr pc, =0x80006d07
1: ldr pc, =0x80006d4b
.ltorg
.global original_text_scroll
.thumb_func
original_text_scroll:
.incbin "main_firmware.bin", 0x61738, 4
ldr pc, =0x8006173d
.ltorg
.global original_scene_clear
.thumb_func
original_scene_clear:
.incbin "main_firmware.bin", 0x5ee0, 4
ldr pc, =0x80005ee5
.ltorg
.global original_callback_unregister
.thumb_func
original_callback_unregister:
.incbin "main_firmware.bin", 0x24eb8, 4
ldr pc, =0x80024ebd
.ltorg
.global original_window_push
.thumb_func
original_window_push:
.incbin "main_firmware.bin", 0x8bd40, 4
ldr pc, =0x8008bd45
.ltorg
.global original_notification_active
.thumb_func
original_notification_active:
.incbin "main_firmware.bin", 0x6060, 4
ldr pc, =0x80006065
.ltorg
.global original_background_draw
.thumb_func
original_background_draw:
.incbin "main_firmware.bin", 0x5e70, 4
ldr pc, =0x80005e75
.ltorg
