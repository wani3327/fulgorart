#!/bin/bash
download_dir=$(mktemp -d)
data_home="${XDG_DATA_HOME:-"$HOME/.local/share"}/fulgorart"
archive=$data_home/$(printf '%s' "$url" | sed -E 's#^[a-zA-Z0-9+.-]*://##; s/[^A-Za-z0-9._-]/_/g')
gallery-dl --download-archive $archive -D $download_dir $url
gallery-dl -J $url | jq '[.[] | select(.[0] == 3)]' | fulgorart-cli $download_dir