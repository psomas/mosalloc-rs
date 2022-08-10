CFLAGS = -O3 -pedantic -Wall -Wextra -march=native -shared -fPIC -DMMAP_CLEARS=O -DUSE_LOCKS=1 -DUSE_SPIN_LOCKS=1

TARGET  = libdlmalloc.so
SOURCE = dlmalloc.c

.PHONY: clean

all: $(TARGET)
clean:
	-rm -f $(TARGET)

$(TARGET): $(SOURCE)
	$(CC) $(CFLAGS) -o $(TARGET) $(SOURCE)
