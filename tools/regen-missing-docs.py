#!/usr/bin/env python3

import base64
import json
import sys
from urllib.request import urlopen, Request

def build_token(token_id, token_secret):
    token = f'{token_id}:{token_secret}'
    token_bytes = token.encode('ascii')
    base64_bytes = base64.b64encode(token_bytes)
    base64_str = base64_bytes.decode('ascii')
    return base64_str

def make_request(method, registry_uri, resource, token):
    url = f'{registry_uri}{resource}'
    with urlopen(Request(url, None, {"Authorization": f'Basic {token}', "Accept": "application/json"}, None, False, method)) as response:
        data = json.load(response)
        return data

def main():
    # read args
    registry_uri = sys.argv[1]
    token_id = sys.argv[2]
    token_secret = sys.argv[3]
    # compute final token
    token = build_token(token_id, token_secret)
    crates = make_request("GET", registry_uri, "/api/v1/crates/undocumented", token)
    for crate in crates:
        crate_name = crate["name"]
        crate_version = crate["version"]
        print(f"missing documentation for: {crate_name} {crate_version}")
        make_request("POST", registry_uri, f"/api/v1/crates/{crate_name}/{crate_version}/docsregen", token)

if __name__ == "__main__":
    main()
