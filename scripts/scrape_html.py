import sys
import struct
from base import run
from bs4 import BeautifulSoup
import re

css_expression = re.compile(r"""url\((?!['"]?(?:data):)['"]?([^'"\)]*)['"]?\)""")

class SimpleScraper:
    def __init__(self, rpc, inp):
        self.rpc = rpc
        self.soup = BeautifulSoup(inp, 'lxml')
    
    def extract_from_attr(self, tag, attr):
        for t in self.soup.find_all(tag):
            if lnk := t.get(attr, None):
                self.rpc.submit(lnk)
    
    def extract_with_generator(self, tag, gen):
        for t in self.soup.find_all(tag):
            for lnk in filter(lambda r: r is not None, gen(t)):
                self.rpc.submit(lnk)

def scrape(rpc, header, inp):
    scraper = SimpleScraper(rpc, inp)

    scraper.extract_from_attr("a", "href")
    scraper.extract_from_attr("link", "href")
    scraper.extract_from_attr("img", "src")
    scraper.extract_from_attr("script", "src")
    scraper.extract_with_generator("style", lambda tag: (link.group(1) for link in css_expression.finditer(tag.string)))

run(scrape)