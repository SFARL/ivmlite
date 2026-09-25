#!/bin/bash

# Prompt the user for the number of files to create
read -p "Enter the number of files to create: " N

# Directory to store the files
OUTPUT_DIR="org_files"
mkdir -p $OUTPUT_DIR

# Loop to create N org files
for ((i=1; i<=N; i++))
do
  # Generate UUIDs
  FILE_UUID=$(uuidgen)
  H1_UUID=$(uuidgen)

  # Create the org file
  FILE_NAME="test_${i}.org"
  cat <<EOL > $OUTPUT_DIR/$FILE_NAME
:PROPERTIES:
:ID: $FILE_UUID
:ROAM_REFS: https://www.example.com
:ROAM_ALIASES: alias_${i}0
:END:
#+TITLE: test_$i
#+FILETAGS: :tag0_$i:

* h1_$i                                         :tag1_$i:tag2_$i:
:PROPERTIES:
:ID: $H1_UUID
:ROAM_ALIASES: alias_${i}1
:END:
EOL

  # Print progress
  echo "Created file $i of $N: $OUTPUT_DIR/$FILE_NAME"
done

echo "Created $N org files in $OUTPUT_DIR directory."
