import json
import urllib.parse
import re
import codecs
from bs4 import BeautifulSoup
from base import RpcException, run
import sys

def flatten(t):
    return [item for sublist in t for item in sublist]

def scrape(rpc, header, inp):
    picrew_id = re.search(r'(?:picrew\.me\/image_maker\/)?(\d+)', header["url"]["url"])
    if not picrew_id:
        return
    
    picrew_id = picrew_id.group(1)

    try:
        soup = BeautifulSoup(inp, features="lxml")
    except:
        return

    for tag in soup.find_all('link'):
        lnk = tag.attrs.get('href')
        if lnk:
            rpc.submit(lnk)

    dyn_load_re = re.compile(r'{([^{]*)}(?=\[e\]\s*?\+\s*?"\.(css|js)")')
    hash_re = re.compile(r'.*?:"(.*?)"')
    base_re = re.compile(r'https:\/\/cdn\.picrew\.me\/assets\/player\/(.+?)\/')

    deploy_key = ""

    dyn_urls = []

    for script in [s['src'] for s in soup.find_all('script') if 'src' in s.attrs]:
        script = urllib.parse.urljoin(f"https://picrew.me/image_maker/{picrew_id}", script)
        try:
            _, script_contents = rpc.fetch(script)
            script_contents = script_contents.read().decode("utf8")
        except RpcException as err:
            print(err, file=sys.stderr)
            continue

        base = base_re.search(script_contents)
        if base != None:
            deploy_key = base.group(1)

        for m in dyn_load_re.finditer(script_contents):
            ext = m.group(2)
            for h_m in hash_re.finditer(m.group(1)):
                dyn_urls.append(h_m.group(1) + '.' + ext)
                
    for url in set(dyn_urls):
        if url.endswith("css"):
            url = f"https://cdn.picrew.me/assets/player/{deploy_key}/css/{url}"
        else:
            url = f"https://cdn.picrew.me/assets/player/{deploy_key}/{url}"

        # if "8694c4d" in url:
            # print(url, file=sys.stderr)
        # print(url, file=sys.stderr)
        # if url.endswith(".")
        rpc.submit(url)


    nuxt_script = str(soup.find("script", string=re.compile("window\.__NUXT__")))
    m = re.search(r'release_key:.*?"(.+?)"', nuxt_script)

    for url in re.finditer('"([^"]+?(?:png|jpg|jpeg))"', nuxt_script):
        rpc.submit(f"https://cdn.picrew.me{codecs.decode(url.group(1), encoding='unicode-escape')}")


    for url in [
        f"https://picrew.me/image_maker/{picrew_id}",
        f"https://cdn.picrew.me/app/image_maker/{picrew_id}/{m.group(1)}/cf.json",
        f"https://cdn.picrew.me/app/image_maker/{picrew_id}/{m.group(1)}/img.json",
        f"https://cdn.picrew.me/app/image_maker/{picrew_id}/{m.group(1)}/scale.json",
        f"https://cdn.picrew.me/app/image_maker/{picrew_id}/{m.group(1)}/i_rule.json",
        "https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.1.1/css/all.min.css",
        "https://fonts.gstatic.com/s/montserrat/v25/JTUSjIg1_i6t8kCHKm459Wlhyw.woff2",
        "https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.1.1/webfonts/fa-brands-400.woff2",
        "https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.1.1/webfonts/fa-v4compatibility.woff2",
        "https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.1.1/webfonts/fa-regular-400.woff2",
        "https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.1.1/webfonts/fa-solid-900.woff2",
        "https://fonts.googleapis.com/css?family=Exo 2",
        "https://fonts.googleapis.com/css2?family=Montserrat:wght@300;400;500;700&display=swap",
        "https://fonts.gstatic.com/s/exo2/v20/7cH1v4okm5zmbvwkAx_sfcEuiD8jvvKsOdC_.woff2",
        "https://api.picrew.me/member/api/profile?lang=en"
    ]:
        rpc.submit(url)
        # pass

run(scrape)