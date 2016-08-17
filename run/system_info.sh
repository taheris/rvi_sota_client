#!/bin/bash

INFO=$(lshw -json -sanitize)

MANIFEST_PATH=/etc/manifest.xml
if [ -f $MANIFEST_PATH ]; then
  MANIFEST={\"manifest_file\":\"$( cat $MANIFEST_PATH | sed 's/"/\\"/'g | sed -e ':a' -e 'N' -e '$!ba' -e 's/\n/ /g' )\"}
  echo $MANIFEST $INFO | jq -s add | jq -r .
else
  echo $INFO | jq -r .
fi