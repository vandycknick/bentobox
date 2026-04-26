#!/usr/bin/env python3

from __future__ import annotations

import copy
from pathlib import Path
from typing import Any

import yaml


SPEC_DIR = Path(__file__).resolve().parent
SWAGGER_PATH = SPEC_DIR / "firecracker-swagger.yaml"
OPENAPI_PATH = SPEC_DIR / "firecracker-openapi.yaml"


def load_yaml(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as handle:
        data = yaml.safe_load(handle)
    if not isinstance(data, dict):
        raise TypeError(f"expected mapping at {path}")
    return data


def rewrite_refs(node: Any) -> Any:
    if isinstance(node, dict):
        rewritten: dict[str, Any] = {}
        for key, value in node.items():
            if key == "$ref" and isinstance(value, str):
                rewritten[key] = value.replace(
                    "#/definitions/", "#/components/schemas/"
                )
            else:
                rewritten[key] = rewrite_refs(value)
        return rewritten
    if isinstance(node, list):
        return [rewrite_refs(item) for item in node]
    return node


def normalize_string_enums(node: Any) -> Any:
    if isinstance(node, dict):
        normalized = {key: normalize_string_enums(value) for key, value in node.items()}
        if normalized.get("type") == "string" and isinstance(normalized.get("enum"), list):
            normalized["enum"] = [str(value) for value in normalized["enum"]]
            if "default" in normalized and normalized["default"] is not None:
                normalized["default"] = str(normalized["default"])
        return normalized
    if isinstance(node, list):
        return [normalize_string_enums(item) for item in node]
    return node


def convert_parameter(parameter: dict[str, Any]) -> dict[str, Any]:
    converted = {
        key: rewrite_refs(copy.deepcopy(value))
        for key, value in parameter.items()
        if key not in {"in", "type", "schema", "items", "collectionFormat"}
    }

    if "in" in parameter:
        converted["in"] = parameter["in"]

    schema = parameter.get("schema")
    if schema is None:
        schema = {}
        if "type" in parameter:
            schema["type"] = parameter["type"]
        if "items" in parameter:
            schema["items"] = rewrite_refs(copy.deepcopy(parameter["items"]))
        if "format" in parameter:
            schema["format"] = parameter["format"]
        if "default" in parameter:
            schema["default"] = copy.deepcopy(parameter["default"])
        if "enum" in parameter:
            schema["enum"] = copy.deepcopy(parameter["enum"])
        if "minimum" in parameter:
            schema["minimum"] = copy.deepcopy(parameter["minimum"])
        if "maximum" in parameter:
            schema["maximum"] = copy.deepcopy(parameter["maximum"])

    if schema:
        converted["schema"] = rewrite_refs(schema)

    return converted


def convert_operation(operation: dict[str, Any], media_types: list[str]) -> dict[str, Any]:
    converted = {
        key: rewrite_refs(copy.deepcopy(value))
        for key, value in operation.items()
        if key not in {"parameters", "responses", "consumes", "produces"}
    }

    body_parameter = None
    parameters: list[dict[str, Any]] = []
    for parameter in operation.get("parameters", []):
        location = parameter.get("in")
        if location == "body":
            body_parameter = parameter
        else:
            parameters.append(convert_parameter(parameter))

    if parameters:
        converted["parameters"] = parameters

    if body_parameter is not None:
        request_media_types = operation.get("consumes") or media_types or ["application/json"]
        converted["requestBody"] = {
            "required": bool(body_parameter.get("required", False)),
            "content": {
                media_type: {
                    "schema": rewrite_refs(copy.deepcopy(body_parameter.get("schema", {})))
                }
                for media_type in request_media_types
            },
        }
        if "description" in body_parameter:
            converted["requestBody"]["description"] = body_parameter["description"]

    responses: dict[str, Any] = {}
    response_media_types = operation.get("produces") or media_types or ["application/json"]
    for status_code, response in operation.get("responses", {}).items():
        converted_response = {
            key: rewrite_refs(copy.deepcopy(value))
            for key, value in response.items()
            if key != "schema"
        }
        if "description" not in converted_response:
            converted_response["description"] = ""

        if "schema" in response:
            converted_response["content"] = {
                media_type: {
                    "schema": rewrite_refs(copy.deepcopy(response["schema"]))
                }
                for media_type in response_media_types
            }

        responses[status_code] = converted_response

    converted["responses"] = responses
    return converted


def convert_document(swagger: dict[str, Any]) -> dict[str, Any]:
    media_types = list(swagger.get("produces") or [])

    openapi: dict[str, Any] = {
        "openapi": "3.0.3",
        "info": rewrite_refs(copy.deepcopy(swagger["info"])),
        "paths": {},
        "components": {
            "schemas": rewrite_refs(copy.deepcopy(swagger.get("definitions", {})))
        },
    }

    schemes = swagger.get("schemes") or ["http"]
    host = swagger.get("host")
    base_path = swagger.get("basePath", "")
    if host:
        openapi["servers"] = [
            {"url": f"{scheme}://{host}{base_path}"} for scheme in schemes
        ]

    for path, path_item in swagger.get("paths", {}).items():
        converted_path_item: dict[str, Any] = {}
        for method, operation in path_item.items():
            if method == "parameters":
                converted_path_item[method] = [
                    convert_parameter(parameter) for parameter in operation
                ]
                continue

            converted_path_item[method] = convert_operation(operation, media_types)
        openapi["paths"][path] = converted_path_item

    return normalize_string_enums(openapi)


def main() -> None:
    swagger = load_yaml(SWAGGER_PATH)
    openapi = convert_document(swagger)

    with OPENAPI_PATH.open("w", encoding="utf-8") as handle:
        yaml.safe_dump(openapi, handle, sort_keys=False)


if __name__ == "__main__":
    main()
