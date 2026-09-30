#include <webp/encode.h>

// Keep libwebp's versioned C structures on the C side of the FFI boundary.
size_t timeline_encode(const unsigned char *bgra, int width, int height,
                       float quality, int method, unsigned char **output) {
    WebPConfig config;
    WebPPicture picture;
    WebPMemoryWriter writer;
    *output = NULL;
    if (!WebPConfigInit(&config) || !WebPPictureInit(&picture)) return 0;
    config.quality = quality;
    config.method = method;
    config.thread_level = 0;
    picture.width = width;
    picture.height = height;
    WebPMemoryWriterInit(&writer);
    picture.writer = WebPMemoryWrite;
    picture.custom_ptr = &writer;
    int ok = WebPValidateConfig(&config) &&
        WebPPictureImportBGRA(&picture, bgra, width * 4) &&
        WebPEncode(&config, &picture);
    WebPPictureFree(&picture);
    if (!ok) { WebPMemoryWriterClear(&writer); return 0; }
    *output = writer.mem;
    return writer.size;
}

size_t timeline_encode_lossless(const unsigned char *bgra, int width, int height,
                                unsigned char **output) {
    WebPConfig config;
    WebPPicture picture;
    WebPMemoryWriter writer;
    *output = NULL;
    if (!WebPConfigInit(&config) || !WebPPictureInit(&picture)) return 0;
    config.lossless = 1;
    config.quality = 0;
    config.method = 0;
    config.exact = 1;
    picture.use_argb = 1;
    picture.width = width;
    picture.height = height;
    WebPMemoryWriterInit(&writer);
    picture.writer = WebPMemoryWrite;
    picture.custom_ptr = &writer;
    int ok = WebPPictureImportBGRA(&picture, bgra, width * 4) && WebPEncode(&config, &picture);
    WebPPictureFree(&picture);
    if (!ok) { WebPMemoryWriterClear(&writer); return 0; }
    *output = writer.mem;
    return writer.size;
}

size_t timeline_thumbnail(const unsigned char *bgra, int width, int height,
                          int target_width, int target_height, unsigned char **output) {
    WebPConfig config;
    WebPPicture picture;
    WebPMemoryWriter writer;
    *output = NULL;
    if (!WebPConfigInit(&config) || !WebPPictureInit(&picture)) return 0;
    config.quality = 80;
    config.method = 0;
    config.thread_level = 0;
    picture.use_argb = 1;
    picture.width = width;
    picture.height = height;
    WebPMemoryWriterInit(&writer);
    picture.writer = WebPMemoryWrite;
    picture.custom_ptr = &writer;
    int ok = WebPPictureImportBGRA(&picture, bgra, width * 4) &&
        WebPPictureRescale(&picture, target_width, target_height) &&
        WebPEncode(&config, &picture);
    WebPPictureFree(&picture);
    if (!ok) { WebPMemoryWriterClear(&writer); return 0; }
    *output = writer.mem;
    return writer.size;
}
