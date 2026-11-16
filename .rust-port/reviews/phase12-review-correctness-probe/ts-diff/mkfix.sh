#!/bin/bash
F=/tmp/review12_correctness/fx
rm -rf $F; mkdir -p $F/dir/sub
printf 'line1\nline2\nline3\n' > $F/a.txt
: > $F/empty.txt
printf 'c1\r\nc2\r\nc3\r\n' > $F/crlf.txt
seq 1 3000 > $F/big.txt
python3 -c "print('x'*100000)" > $F/longline.txt
printf '\x00\x01\x02binary\xff\xfe' > $F/bin.dat
printf 'caf\xe9 latin1\n' > $F/latin1.txt
printf 'no newline at end' > $F/nonl.txt
printf 'emoji \xf0\x9f\x98\x80 line\nsecond \xe2\x82\xac\n' > $F/emoji.txt
printf '\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89' > $F/img.png
printf 'aaa bbb aaa\nccc\n' > $F/edit.txt
printf 'one two three\n' > $F/edit2.txt
printf 'x\r\ny\r\n' > $F/editcrlf.txt
printf '%%PDF-1.4 fake' > $F/doc.pdf
printf 'only\n\n\n' > $F/blank.txt
