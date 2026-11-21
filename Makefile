# ivitals dashboard commands.
#
# Safety: `clean` only removes dashboard build artifacts. It never touches
# data/, which is the daemon's read-only source of truth.

DASHBOARD := dashboard

.PHONY: install dev build start clean

install:
	cd $(DASHBOARD) && bun install

dev:
	cd $(DASHBOARD) && bun run dev

build: clean
	cd $(DASHBOARD) && bun run build

start:
	cd $(DASHBOARD) && bun run start

clean:
	@echo "removing dashboard build artifacts only (data/ is untouched)"
	rm -rf $(DASHBOARD)/dist
