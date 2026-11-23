# fulgorart-gdl: gallery-dl in a container, kept separate from the CLI image so
# it can be updated on its own schedule and only this container sees the
# gallery-dl config (site tokens/cookies), which is mounted at runtime and
# never copied into the image (the image is public).
FROM python:3.12-slim

# Bump deliberately (sites change and gallery-dl ships extractor fixes often).
ARG GALLERY_DL_VERSION=1.32.8
RUN pip install --no-cache-dir "gallery-dl==${GALLERY_DL_VERSION}"

# Config at /config/gallery-dl/config.json (mount read-only); gallery-dl keeps
# refreshed tokens in /cache (mount writable).
ENV XDG_CONFIG_HOME=/config \
    XDG_CACHE_HOME=/cache

WORKDIR /staging
ENTRYPOINT ["gallery-dl"]
