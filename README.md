# mosalloc-rs
 A Rust rewrite of [mosalloc](https://github.com/technion-csl/mosalloc).

## How to run
```
git clone https://github.com/psomas/mosalloc-rs.git

cd mosalloc-rs

cat <<EOF > cpf.csv
type,page_size,start_offset,end_offset
mmap,2MB,5GB,6GB
mmap,1GB,0,4GB
brk,1GB,0,10GB
mmap,64KB,8GB,9GB
EOF

cargo build --workspace -r
./target/release/run_mosalloc --lib ./target/release/libmosalloc.so --config cpf.csv -- ls
```

## Changes from original mosalloc
TODO
