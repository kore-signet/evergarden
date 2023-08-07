use lazy_regex::regex;
use url::{Host, Url};

pub fn surt(mut url: Url) -> String {
    if let Some(Host::Domain(s)) = url.host() {
        #[allow(unused_must_use)]
        if let Some(mat) = regex!(r#"^www\d*\."#).find(s) {
            url.set_host(Some(&s[mat.end()..].to_owned()));
        }
    }

    let mut surt = String::with_capacity(url.as_str().len());
    let mut part_iter = url.host_str().map_or("".rsplit('.'), |v| v.rsplit('.'));

    if let Some(part) = part_iter.next() {
        surt.push_str(part);
        part_iter.for_each(|v| {
            surt.push(',');
            surt.push_str(v);
        })
    }

    let mut itoa_buffer = itoa::Buffer::new();

    if let Some(prt) = url.port() {
        surt.push(':');
        surt.push_str(itoa_buffer.format(prt));
    }

    surt.push(')');
    surt.push_str(url.path());

    let mut sorted_pairs = url
        .query_pairs()
        .map(|(a, b)| (a.into_owned().to_lowercase(), b.into_owned().to_lowercase()))
        .collect::<Vec<(String, String)>>();
    sorted_pairs.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
    url.query_pairs_mut()
        .clear()
        .extend_pairs(&sorted_pairs)
        .finish();

    if let Some(q) = url.query().filter(|q| !q.is_empty()) {
        surt.push('?');
        surt.push_str(q);
    }

    surt
}

#[cfg(test)]
mod tests {
    #[test]
    fn url_to_surt() {
        macro_rules! test {
            ($a:literal, $b:literal) => {
                let url = url::Url::parse($a).unwrap();
                assert_eq!(super::surt(url).as_str(), $b);
            };
        }

        test!(
            "https://www23.example.com/some/path",
            "com,example)/some/path"
        );
        test!(
            "https://example.com/www2.example/some/value",
            "com,example)/www2.example/some/value"
        );
        test!(
            "https://abc.www.example.com/example",
            "com,example,www,abc)/example"
        );
        test!(
            "https://www.example.com:443/some/path",
            "com,example)/some/path"
        );
        test!(
            "http://www.example.com:80/some/path",
            "com,example)/some/path"
        );
        test!(
            "https://www.example.com:123/some/path",
            "com,example:123)/some/path"
        );
        test!(
            "https://www.example.com/some/path?D=1&CC=2&EE=3",
            "com,example)/some/path?cc=2&d=1&ee=3"
        );
        test!(
            "https://www.example.com/some/path?a=b&c&cc=1&d=e",
            "com,example)/some/path?a=b&c=&cc=1&d=e"
        );
    }
}
