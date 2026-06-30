#ifndef RCHAT_VPX_H
#define RCHAT_VPX_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct RchatVpxEncoder RchatVpxEncoder;
typedef struct RchatVpxDecoder RchatVpxDecoder;

typedef struct RchatVpxPacket {
    uint8_t *data;
    size_t len;
    int is_key;
} RchatVpxPacket;

typedef struct RchatVpxPacketList {
    RchatVpxPacket *packets;
    size_t len;
} RchatVpxPacketList;

typedef struct RchatVpxDecodedFrame {
    uint8_t *data;
    size_t len;
    uint32_t width;
    uint32_t height;
} RchatVpxDecodedFrame;

enum {
    RCHAT_VPX_OK = 0,
    RCHAT_VPX_INVALID_ARGUMENT = 1,
    RCHAT_VPX_ALLOC_FAILED = 2,
    RCHAT_VPX_ENCODER_CONFIG_FAILED = 3,
    RCHAT_VPX_ENCODER_INIT_FAILED = 4,
    RCHAT_VPX_ENCODER_CONTROL_FAILED = 5,
    RCHAT_VPX_ENCODE_FAILED = 6,
    RCHAT_VPX_DECODER_INIT_FAILED = 7,
    RCHAT_VPX_DECODE_FAILED = 8,
    RCHAT_VPX_NO_DECODED_FRAME = 9
};

int rchat_vpx_encoder_new(
    uint32_t width,
    uint32_t height,
    uint32_t bitrate_kbps,
    uint32_t fps,
    uint32_t threads,
    uint32_t keyframe_interval,
    int32_t cpu_used,
    RchatVpxEncoder **out_encoder);

void rchat_vpx_encoder_free(RchatVpxEncoder *encoder);

int rchat_vpx_encoder_encode_i420(
    RchatVpxEncoder *encoder,
    const uint8_t *data,
    size_t data_len,
    int force_keyframe,
    RchatVpxPacketList *out_packets);

void rchat_vpx_packet_list_free(RchatVpxPacketList *packets);

int rchat_vpx_decoder_new(RchatVpxDecoder **out_decoder);

void rchat_vpx_decoder_free(RchatVpxDecoder *decoder);

int rchat_vpx_decoder_decode_i420(
    RchatVpxDecoder *decoder,
    const uint8_t *data,
    size_t data_len,
    RchatVpxDecodedFrame *out_frame);

void rchat_vpx_decoded_frame_free(RchatVpxDecodedFrame *frame);

int rchat_vpx_probe_vp8_decode(const uint8_t *data, size_t data_len);

const char *rchat_vpx_status_message(int status);

#ifdef __cplusplus
}
#endif

#endif
