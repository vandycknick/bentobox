.PHONY: build
build:
	docker buildx build \
		--platform linux/arm64/v8 \
		--load -f Containerfile \
		--tag ghcr.io/vandycknick/archlinuxarm:latest .

