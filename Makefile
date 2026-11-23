# ivitals dashboard commands.
#
# Safety: `clean` only removes dashboard build artifacts. It never touches
# data/, which is the daemon's read-only source of truth.

DASHBOARD := dashboard

.PHONY: install dev build start start-bg stop clean collector-start collector-stop

install:
	cd $(DASHBOARD) && bun install

dev:
	cd $(DASHBOARD) && bun run dev

build: clean
	cd $(DASHBOARD) && bun run build

start:
	cd $(DASHBOARD) && bun run start

# Detached production server that survives the launching terminal.
start-bg: build
	@cd $(DASHBOARD) && setsid nohup bun src/server.ts > ivitals.log 2>&1 & echo $$! > $(DASHBOARD)/ivitals.pid
	@sleep 1
	@echo "dashboard started (pid $$(cat $(DASHBOARD)/ivitals.pid)); log: $(DASHBOARD)/ivitals.log"

stop:
	@if [ -f $(DASHBOARD)/ivitals.pid ]; then \
		kill $$(cat $(DASHBOARD)/ivitals.pid) 2>/dev/null || true; \
		rm -f $(DASHBOARD)/ivitals.pid; \
		echo "dashboard stopped"; \
	else \
		echo "no pidfile; use: pkill -f 'bun src/server.ts'"; \
	fi

clean:
	@echo "removing dashboard build artifacts only (data/ is untouched)"
	rm -rf $(DASHBOARD)/dist

collector-start:
	@pgrep -f '^./target/release/ivitals$$' >/dev/null && echo "collector already running" || (setsid nohup ./target/release/ivitals > ivitals.log 2>&1 &)
	@sleep 1
	@pgrep -f '^./target/release/ivitals$$' > ivitals.pid && echo "collector pid $$(cat ivitals.pid); log: ivitals.log"

collector-stop:
	@if [ -f ivitals.pid ]; then kill -INT $$(cat ivitals.pid) 2>/dev/null || true; rm -f ivitals.pid; fi
	@for i in 1 2 3 4 5 6 7 8 9 10; do pgrep -f '^./target/release/ivitals$$' >/dev/null || break; sleep 0.3; done
	@pgrep -f '^./target/release/ivitals$$' >/dev/null && echo "collector still running" || echo "collector stopped"
