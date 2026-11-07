#!/bin/bash

mkdir -p /tmp/project-extract && \
unzip -oq ~/Downloads/st-cli.zip -d /tmp/project-extract && \
rsync -av --exclude=".git" --exclude=".gitignore" /tmp/project-extract/*/ ./
