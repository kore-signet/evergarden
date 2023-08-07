import re
import sys
from base import run

url_expression = re.compile(r"""url\((?!['"]?(?:data):)['"]?([^'"\)]*)['"]?\)""")

def scrape(rpc, header, inp):
    inp = inp.read().decode("utf8")
    for match in url_expression.finditer(inp):
        rpc.submit(match.group(1))

run(scrape)