// Per-window capture: one ext_image_copy_capture session, two shm buffers.
// Frames are captured into the back buffer; on ready the buffer is handed to
// the overlay (attached to the thumbnail subsurface) and the buffers swap.
// In live mode the next frame is requested right away; the compositor only
// answers once the window has new content.

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "app.h"
#include "ext-foreign-toplevel-list-v1-client-protocol.h"
#include "ext-image-capture-source-v1-client-protocol.h"
#include "ext-image-copy-capture-v1-client-protocol.h"

#define MAX_FAILURES 3

static void request_frame(struct capture *cap);

// ---------------------------------------------------------------------------
// shm buffers

static void buffer_release(void *data, struct wl_buffer *wl_buffer) {
	(void)wl_buffer;
	struct shm_buffer *buf = data;
	buf->busy = false;
	request_frame(buf->cap); // in case the capture was waiting for this buffer
}

static const struct wl_buffer_listener buffer_listener = {
	.release = buffer_release,
};

static bool shm_buffer_create(struct capture *cap, struct shm_buffer *buf,
		uint32_t width, uint32_t height, uint32_t format) {
	struct wl_shm *shm = cap->win->app->g.shm;
	*buf = (struct shm_buffer){
		.cap = cap,
		.width = width,
		.height = height,
		.stride = width * 4,
		.format = format,
	};
	buf->size = (size_t)buf->stride * height;

	int fd = memfd_create("wl-expose-shm", MFD_CLOEXEC);
	if (fd < 0) {
		perror("wl-expose: memfd_create");
		return false;
	}
	if (ftruncate(fd, (off_t)buf->size) < 0) {
		perror("wl-expose: ftruncate");
		close(fd);
		return false;
	}
	buf->data = mmap(NULL, buf->size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (buf->data == MAP_FAILED) {
		perror("wl-expose: mmap");
		buf->data = NULL;
		close(fd);
		return false;
	}

	struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, (int32_t)buf->size);
	buf->buffer = wl_shm_pool_create_buffer(pool, 0, (int32_t)width,
		(int32_t)height, (int32_t)buf->stride, format);
	wl_buffer_add_listener(buf->buffer, &buffer_listener, buf);
	wl_shm_pool_destroy(pool);
	close(fd);
	return true;
}

static void shm_buffer_destroy(struct shm_buffer *buf) {
	// Destroying a buffer that is still attached is fine: the compositor
	// keeps showing its last contents until the next attach.
	if (buf->buffer)
		wl_buffer_destroy(buf->buffer);
	if (buf->data)
		munmap(buf->data, buf->size);
	memset(buf, 0, sizeof(*buf));
}

// ---------------------------------------------------------------------------
// state transitions

static void capture_give_up(struct capture *cap, const char *why) {
	struct window *win = cap->win;
	cap->done = true;
	if (cap->frame) {
		ext_image_copy_capture_frame_v1_destroy(cap->frame);
		cap->frame = NULL;
	}
	if (!cap->have_frame) {
		fprintf(stderr, "wl-expose: capture of '%s' failed: %s\n",
			win->tl->identifier ? win->tl->identifier : "", why);
		cap->failed = true;
		overlay_window_failed(win);
	}
}

static bool shm_format_supported(uint32_t format) {
	switch (format) {
	case WL_SHM_FORMAT_ARGB8888:
	case WL_SHM_FORMAT_XRGB8888:
	case WL_SHM_FORMAT_ABGR8888:
	case WL_SHM_FORMAT_XBGR8888:
		return true;
	default:
		return false;
	}
}

// (Re)allocate buffers for the latest constraints and request a frame.
static void apply_constraints(struct capture *cap) {
	uint32_t format = 0;
	bool found = false;
	for (size_t i = 0; i < cap->n_shm_formats && !found; i++) {
		if (shm_format_supported(cap->shm_formats[i])) {
			format = cap->shm_formats[i];
			found = true;
		}
	}
	if (!found) {
		capture_give_up(cap, "no supported shm format");
		return;
	}
	if (cap->width == 0 || cap->height == 0) {
		capture_give_up(cap, "zero buffer size");
		return;
	}

	struct shm_buffer *b = &cap->bufs[0];
	if (!b->buffer || b->width != cap->width || b->height != cap->height ||
			b->format != format) {
		for (int i = 0; i < 2; i++) {
			shm_buffer_destroy(&cap->bufs[i]);
			if (!shm_buffer_create(cap, &cap->bufs[i], cap->width, cap->height, format)) {
				capture_give_up(cap, "shm allocation failed");
				return;
			}
		}
		cap->back = 0;
	}
	cap->have_constraints = true;
	request_frame(cap);
}

static void after_frame(struct capture *cap) {
	if (cap->realloc_pending) {
		cap->realloc_pending = false;
		apply_constraints(cap);
	} else {
		request_frame(cap);
	}
}

// ---------------------------------------------------------------------------
// frame

static void frame_transform(void *data, struct ext_image_copy_capture_frame_v1 *f,
		uint32_t transform) {
	(void)f;
	((struct capture *)data)->transform = transform;
}

static void frame_damage(void *data, struct ext_image_copy_capture_frame_v1 *f,
		int32_t x, int32_t y, int32_t w, int32_t h) {
	(void)data;
	(void)f;
	(void)x;
	(void)y;
	(void)w;
	(void)h;
}

static void frame_presentation_time(void *data, struct ext_image_copy_capture_frame_v1 *f,
		uint32_t sec_hi, uint32_t sec_lo, uint32_t nsec) {
	(void)data;
	(void)f;
	(void)sec_hi;
	(void)sec_lo;
	(void)nsec;
}

static void frame_ready(void *data, struct ext_image_copy_capture_frame_v1 *f) {
	struct capture *cap = data;
	ext_image_copy_capture_frame_v1_destroy(f);
	cap->frame = NULL;
	cap->failures = 0;

	struct shm_buffer *buf = &cap->bufs[cap->back];
	cap->back ^= 1;
	cap->have_frame = true;
	if (!cap->win->app->opts.live)
		cap->done = true;
	overlay_window_frame(cap->win, buf, cap->transform);
	after_frame(cap);
}

static void frame_failed(void *data, struct ext_image_copy_capture_frame_v1 *f,
		uint32_t reason) {
	struct capture *cap = data;
	ext_image_copy_capture_frame_v1_destroy(f);
	cap->frame = NULL;

	if (reason == EXT_IMAGE_COPY_CAPTURE_FRAME_V1_FAILURE_REASON_STOPPED) {
		capture_give_up(cap, "stopped");
		return;
	}
	if (++cap->failures > MAX_FAILURES) {
		capture_give_up(cap,
			reason == EXT_IMAGE_COPY_CAPTURE_FRAME_V1_FAILURE_REASON_BUFFER_CONSTRAINTS
				? "buffer constraints" : "unknown");
		return;
	}
	// On buffer_constraints the session sends new constraints; if they
	// arrived already, after_frame reallocates. Otherwise retry.
	after_frame(cap);
}

static const struct ext_image_copy_capture_frame_v1_listener frame_listener = {
	.transform = frame_transform,
	.damage = frame_damage,
	.presentation_time = frame_presentation_time,
	.ready = frame_ready,
	.failed = frame_failed,
};

static void request_frame(struct capture *cap) {
	if (cap->frame || cap->done || !cap->session || !cap->have_constraints ||
			cap->realloc_pending)
		return;
	struct shm_buffer *buf = &cap->bufs[cap->back];
	if (!buf->buffer || buf->busy)
		return; // resumed from buffer_release

	cap->transform = WL_OUTPUT_TRANSFORM_NORMAL;
	cap->frame = ext_image_copy_capture_session_v1_create_frame(cap->session);
	ext_image_copy_capture_frame_v1_add_listener(cap->frame, &frame_listener, cap);
	ext_image_copy_capture_frame_v1_attach_buffer(cap->frame, buf->buffer);
	ext_image_copy_capture_frame_v1_damage_buffer(cap->frame, 0, 0,
		(int32_t)buf->width, (int32_t)buf->height);
	ext_image_copy_capture_frame_v1_capture(cap->frame);
}

// ---------------------------------------------------------------------------
// session

static void begin_constraint_event(struct capture *cap) {
	if (cap->constraints_reset) {
		cap->n_shm_formats = 0;
		cap->constraints_reset = false;
	}
}

static void session_buffer_size(void *data, struct ext_image_copy_capture_session_v1 *s,
		uint32_t width, uint32_t height) {
	(void)s;
	struct capture *cap = data;
	begin_constraint_event(cap);
	cap->width = width;
	cap->height = height;
}

static void session_shm_format(void *data, struct ext_image_copy_capture_session_v1 *s,
		uint32_t format) {
	(void)s;
	struct capture *cap = data;
	begin_constraint_event(cap);
	if (cap->n_shm_formats < MAX_SHM_FORMATS)
		cap->shm_formats[cap->n_shm_formats++] = format;
}

static void session_dmabuf_device(void *data, struct ext_image_copy_capture_session_v1 *s,
		struct wl_array *device) {
	(void)s;
	(void)device;
	begin_constraint_event(data);
}

static void session_dmabuf_format(void *data, struct ext_image_copy_capture_session_v1 *s,
		uint32_t format, struct wl_array *modifiers) {
	(void)s;
	(void)format;
	(void)modifiers;
	begin_constraint_event(data);
}

static void session_done(void *data, struct ext_image_copy_capture_session_v1 *s) {
	(void)s;
	struct capture *cap = data;
	cap->constraints_reset = true;
	if (cap->done)
		return;
	if (cap->frame) {
		// The in-flight frame will fail or complete; reallocate after it.
		cap->realloc_pending = true;
		return;
	}
	apply_constraints(cap);
}

static void session_stopped(void *data, struct ext_image_copy_capture_session_v1 *s) {
	(void)s;
	struct capture *cap = data;
	capture_give_up(cap, "session stopped");
	ext_image_copy_capture_session_v1_destroy(cap->session);
	cap->session = NULL;
}

static const struct ext_image_copy_capture_session_v1_listener session_listener = {
	.buffer_size = session_buffer_size,
	.shm_format = session_shm_format,
	.dmabuf_device = session_dmabuf_device,
	.dmabuf_format = session_dmabuf_format,
	.done = session_done,
	.stopped = session_stopped,
};

// ---------------------------------------------------------------------------

void capture_start(struct window *win) {
	struct app *app = win->app;
	struct capture *cap = &win->cap;
	cap->win = win;
	cap->source = ext_foreign_toplevel_image_capture_source_manager_v1_create_source(
		app->g.source_manager, win->tl->handle);
	cap->session = ext_image_copy_capture_manager_v1_create_session(
		app->g.copy_manager, cap->source,
		app->opts.cursors ? EXT_IMAGE_COPY_CAPTURE_MANAGER_V1_OPTIONS_PAINT_CURSORS : 0);
	ext_image_copy_capture_session_v1_add_listener(cap->session, &session_listener, cap);
}

void capture_finish(struct capture *cap) {
	if (cap->frame)
		ext_image_copy_capture_frame_v1_destroy(cap->frame);
	if (cap->session)
		ext_image_copy_capture_session_v1_destroy(cap->session);
	if (cap->source)
		ext_image_capture_source_v1_destroy(cap->source);
	shm_buffer_destroy(&cap->bufs[0]);
	shm_buffer_destroy(&cap->bufs[1]);
	cap->frame = NULL;
	cap->session = NULL;
	cap->source = NULL;
}

bool capture_settled(const struct capture *cap) {
	return cap->have_frame || cap->failed;
}

