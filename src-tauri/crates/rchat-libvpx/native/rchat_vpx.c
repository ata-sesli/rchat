#include "rchat_vpx.h"

#include <stdlib.h>
#include <string.h>

#include <vpx/vp8cx.h>
#include <vpx/vp8dx.h>
#include <vpx/vpx_decoder.h>
#include <vpx/vpx_encoder.h>
#include <vpx/vpx_image.h>

struct RchatVpxEncoder {
    vpx_codec_ctx_t codec;
    vpx_codec_enc_cfg_t config;
    uint32_t width;
    uint32_t height;
    int initialized;
    int64_t frame_index;
};

struct RchatVpxDecoder {
    vpx_codec_ctx_t codec;
    int initialized;
};

static size_t rchat_vpx_expected_i420_len(uint32_t width, uint32_t height) {
    size_t pixels = (size_t)width * (size_t)height;
    return pixels + pixels / 2;
}

static void rchat_vpx_packet_list_clear(RchatVpxPacketList *packets) {
    if (packets == NULL) {
        return;
    }
    if (packets->packets != NULL) {
        for (size_t i = 0; i < packets->len; i++) {
            free(packets->packets[i].data);
        }
        free(packets->packets);
    }
    packets->packets = NULL;
    packets->len = 0;
}

static int rchat_vpx_append_packet(
    RchatVpxPacketList *packets,
    const uint8_t *data,
    size_t len,
    int is_key) {
    RchatVpxPacket *next =
        (RchatVpxPacket *)realloc(packets->packets, (packets->len + 1) * sizeof(RchatVpxPacket));
    if (next == NULL) {
        rchat_vpx_packet_list_clear(packets);
        return RCHAT_VPX_ALLOC_FAILED;
    }

    packets->packets = next;
    packets->packets[packets->len].data = NULL;
    packets->packets[packets->len].len = 0;
    packets->packets[packets->len].is_key = is_key;

    if (len > 0) {
        packets->packets[packets->len].data = (uint8_t *)malloc(len);
        if (packets->packets[packets->len].data == NULL) {
            rchat_vpx_packet_list_clear(packets);
            return RCHAT_VPX_ALLOC_FAILED;
        }
        memcpy(packets->packets[packets->len].data, data, len);
    }

    packets->packets[packets->len].len = len;
    packets->len += 1;
    return RCHAT_VPX_OK;
}

int rchat_vpx_encoder_new(
    uint32_t width,
    uint32_t height,
    uint32_t bitrate_kbps,
    uint32_t fps,
    uint32_t threads,
    uint32_t keyframe_interval,
    int32_t cpu_used,
    RchatVpxEncoder **out_encoder) {
    if (out_encoder == NULL) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }
    *out_encoder = NULL;

    if (width == 0 || height == 0 || (width % 2) != 0 || (height % 2) != 0 || fps == 0 ||
        bitrate_kbps == 0 || keyframe_interval == 0) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }

    RchatVpxEncoder *encoder = (RchatVpxEncoder *)calloc(1, sizeof(RchatVpxEncoder));
    if (encoder == NULL) {
        return RCHAT_VPX_ALLOC_FAILED;
    }

    if (vpx_codec_enc_config_default(vpx_codec_vp8_cx(), &encoder->config, 0) != VPX_CODEC_OK) {
        free(encoder);
        return RCHAT_VPX_ENCODER_CONFIG_FAILED;
    }

    encoder->config.g_w = width;
    encoder->config.g_h = height;
    encoder->config.g_timebase.num = 1;
    encoder->config.g_timebase.den = (int)fps;
    encoder->config.g_threads = threads == 0 ? 1 : threads;
    encoder->config.g_lag_in_frames = 0;
    encoder->config.rc_target_bitrate = bitrate_kbps;
    encoder->config.rc_dropframe_thresh = 0;
    encoder->config.rc_resize_allowed = 0;
    encoder->config.kf_mode = VPX_KF_AUTO;
    encoder->config.kf_min_dist = keyframe_interval;
    encoder->config.kf_max_dist = keyframe_interval;

    if (vpx_codec_enc_init(&encoder->codec, vpx_codec_vp8_cx(), &encoder->config, 0) !=
        VPX_CODEC_OK) {
        free(encoder);
        return RCHAT_VPX_ENCODER_INIT_FAILED;
    }
    encoder->initialized = 1;

    if (vpx_codec_control(&encoder->codec, VP8E_SET_CPUUSED, cpu_used) != VPX_CODEC_OK) {
        rchat_vpx_encoder_free(encoder);
        return RCHAT_VPX_ENCODER_CONTROL_FAILED;
    }
    if (vpx_codec_control(&encoder->codec, VP8E_SET_MAX_INTRA_BITRATE_PCT, 450) !=
        VPX_CODEC_OK) {
        rchat_vpx_encoder_free(encoder);
        return RCHAT_VPX_ENCODER_CONTROL_FAILED;
    }

    encoder->width = width;
    encoder->height = height;
    encoder->frame_index = 0;
    *out_encoder = encoder;
    return RCHAT_VPX_OK;
}

void rchat_vpx_encoder_free(RchatVpxEncoder *encoder) {
    if (encoder == NULL) {
        return;
    }
    if (encoder->initialized) {
        vpx_codec_destroy(&encoder->codec);
    }
    free(encoder);
}

int rchat_vpx_encoder_encode_i420(
    RchatVpxEncoder *encoder,
    const uint8_t *data,
    size_t data_len,
    int force_keyframe,
    RchatVpxPacketList *out_packets) {
    if (out_packets == NULL) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }
    out_packets->packets = NULL;
    out_packets->len = 0;

    if (encoder == NULL || data == NULL) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }
    if (data_len != rchat_vpx_expected_i420_len(encoder->width, encoder->height)) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }

    vpx_image_t image;
    if (vpx_img_wrap(
            &image,
            VPX_IMG_FMT_I420,
            encoder->width,
            encoder->height,
            1,
            (unsigned char *)data) == NULL) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }

    vpx_enc_frame_flags_t flags = force_keyframe ? VPX_EFLAG_FORCE_KF : 0;
    if (vpx_codec_encode(
            &encoder->codec,
            &image,
            encoder->frame_index,
            1,
            flags,
            VPX_DL_REALTIME) != VPX_CODEC_OK) {
        return RCHAT_VPX_ENCODE_FAILED;
    }
    encoder->frame_index += 1;

    vpx_codec_iter_t iter = NULL;
    const vpx_codec_cx_pkt_t *packet = NULL;
    while ((packet = vpx_codec_get_cx_data(&encoder->codec, &iter)) != NULL) {
        if (packet->kind != VPX_CODEC_CX_FRAME_PKT) {
            continue;
        }

        int status = rchat_vpx_append_packet(
            out_packets,
            (const uint8_t *)packet->data.frame.buf,
            (size_t)packet->data.frame.sz,
            (packet->data.frame.flags & VPX_FRAME_IS_KEY) != 0);
        if (status != RCHAT_VPX_OK) {
            return status;
        }
    }

    return RCHAT_VPX_OK;
}

void rchat_vpx_packet_list_free(RchatVpxPacketList *packets) {
    rchat_vpx_packet_list_clear(packets);
}

int rchat_vpx_decoder_new(RchatVpxDecoder **out_decoder) {
    if (out_decoder == NULL) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }
    *out_decoder = NULL;

    RchatVpxDecoder *decoder = (RchatVpxDecoder *)calloc(1, sizeof(RchatVpxDecoder));
    if (decoder == NULL) {
        return RCHAT_VPX_ALLOC_FAILED;
    }

    if (vpx_codec_dec_init(&decoder->codec, vpx_codec_vp8_dx(), NULL, 0) != VPX_CODEC_OK) {
        free(decoder);
        return RCHAT_VPX_DECODER_INIT_FAILED;
    }

    decoder->initialized = 1;
    *out_decoder = decoder;
    return RCHAT_VPX_OK;
}

void rchat_vpx_decoder_free(RchatVpxDecoder *decoder) {
    if (decoder == NULL) {
        return;
    }
    if (decoder->initialized) {
        vpx_codec_destroy(&decoder->codec);
    }
    free(decoder);
}

static int rchat_vpx_copy_decoded_i420(vpx_image_t *image, RchatVpxDecodedFrame *out_frame) {
    if (image == NULL || out_frame == NULL || image->fmt != VPX_IMG_FMT_I420) {
        return RCHAT_VPX_DECODE_FAILED;
    }

    uint32_t width = image->d_w;
    uint32_t height = image->d_h;
    if (width == 0 || height == 0 || (width % 2) != 0 || (height % 2) != 0) {
        return RCHAT_VPX_DECODE_FAILED;
    }

    size_t y_len = (size_t)width * (size_t)height;
    size_t uv_width = (size_t)width / 2;
    size_t uv_height = (size_t)height / 2;
    size_t uv_len = uv_width * uv_height;
    size_t len = y_len + uv_len * 2;
    uint8_t *data = (uint8_t *)malloc(len);
    if (data == NULL) {
        return RCHAT_VPX_ALLOC_FAILED;
    }

    for (uint32_t row = 0; row < height; row++) {
        memcpy(
            data + (size_t)row * width,
            image->planes[VPX_PLANE_Y] + row * image->stride[VPX_PLANE_Y],
            width);
    }

    size_t u_offset = y_len;
    size_t v_offset = y_len + uv_len;
    for (uint32_t row = 0; row < height / 2; row++) {
        memcpy(
            data + u_offset + (size_t)row * uv_width,
            image->planes[VPX_PLANE_U] + row * image->stride[VPX_PLANE_U],
            uv_width);
        memcpy(
            data + v_offset + (size_t)row * uv_width,
            image->planes[VPX_PLANE_V] + row * image->stride[VPX_PLANE_V],
            uv_width);
    }

    out_frame->data = data;
    out_frame->len = len;
    out_frame->width = width;
    out_frame->height = height;
    return RCHAT_VPX_OK;
}

int rchat_vpx_decoder_decode_i420(
    RchatVpxDecoder *decoder,
    const uint8_t *data,
    size_t data_len,
    RchatVpxDecodedFrame *out_frame) {
    if (out_frame == NULL) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }
    out_frame->data = NULL;
    out_frame->len = 0;
    out_frame->width = 0;
    out_frame->height = 0;

    if (decoder == NULL || data == NULL || data_len == 0) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }

    if (vpx_codec_decode(&decoder->codec, data, (unsigned int)data_len, NULL, 0) !=
        VPX_CODEC_OK) {
        return RCHAT_VPX_DECODE_FAILED;
    }

    vpx_codec_iter_t iter = NULL;
    vpx_image_t *image = vpx_codec_get_frame(&decoder->codec, &iter);
    if (image == NULL) {
        return RCHAT_VPX_NO_DECODED_FRAME;
    }
    return rchat_vpx_copy_decoded_i420(image, out_frame);
}

void rchat_vpx_decoded_frame_free(RchatVpxDecodedFrame *frame) {
    if (frame == NULL) {
        return;
    }
    free(frame->data);
    frame->data = NULL;
    frame->len = 0;
    frame->width = 0;
    frame->height = 0;
}

int rchat_vpx_probe_vp8_decode(const uint8_t *data, size_t data_len) {
    if (data == NULL || data_len == 0) {
        return RCHAT_VPX_INVALID_ARGUMENT;
    }

    vpx_codec_ctx_t decoder;
    if (vpx_codec_dec_init(&decoder, vpx_codec_vp8_dx(), NULL, 0) != VPX_CODEC_OK) {
        return RCHAT_VPX_DECODER_INIT_FAILED;
    }

    if (vpx_codec_decode(&decoder, data, (unsigned int)data_len, NULL, 0) != VPX_CODEC_OK) {
        vpx_codec_destroy(&decoder);
        return RCHAT_VPX_DECODE_FAILED;
    }

    vpx_codec_iter_t iter = NULL;
    vpx_image_t *frame = vpx_codec_get_frame(&decoder, &iter);
    int status = frame == NULL ? RCHAT_VPX_NO_DECODED_FRAME : RCHAT_VPX_OK;
    vpx_codec_destroy(&decoder);
    return status;
}

const char *rchat_vpx_status_message(int status) {
    switch (status) {
    case RCHAT_VPX_OK:
        return "ok";
    case RCHAT_VPX_INVALID_ARGUMENT:
        return "invalid argument";
    case RCHAT_VPX_ALLOC_FAILED:
        return "allocation failed";
    case RCHAT_VPX_ENCODER_CONFIG_FAILED:
        return "libvpx encoder config failed";
    case RCHAT_VPX_ENCODER_INIT_FAILED:
        return "libvpx encoder init failed";
    case RCHAT_VPX_ENCODER_CONTROL_FAILED:
        return "libvpx encoder control failed";
    case RCHAT_VPX_ENCODE_FAILED:
        return "libvpx encode failed";
    case RCHAT_VPX_DECODER_INIT_FAILED:
        return "libvpx decoder init failed";
    case RCHAT_VPX_DECODE_FAILED:
        return "libvpx decode failed";
    case RCHAT_VPX_NO_DECODED_FRAME:
        return "libvpx produced no decoded frame";
    default:
        return "unknown libvpx error";
    }
}
