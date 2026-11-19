//! Offset paging for a person's own lists: `?offset=&limit=` in, `{items, more}`
//! out. A page fetches one row more than it shows, so `more` is exact without a
//! count. Admin and audit lists, which can be long, page by cursor instead.

use serde::Deserialize;

/// The page a list request asks for, read with `Query<PageQuery>` or flattened
/// into a wider query struct.
#[derive(Deserialize, Clone, Copy, Debug, Default)]
pub struct PageQuery {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

impl PageQuery {
    /// The page, with `default` rows when the request names no limit.
    pub fn page(self, default: i64) -> Page {
        Page {
            offset: self.offset.unwrap_or(0).clamp(0, Page::FARTHEST),
            limit: self.limit.unwrap_or(default).clamp(1, Page::MOST),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub offset: i64,
    pub limit: i64,
}

impl Page {
    /// The most rows one page shows: a person's largest list (2,000 documents in
    /// a collection) fits in one page when they ask for it.
    pub const MOST: i64 = 2000;
    const FARTHEST: i64 = 100_000;

    /// Rows to fetch: one more than the page shows, which says whether more follow.
    pub fn fetch(self) -> i64 {
        self.limit + 1
    }

    /// From `fetch()` rows: the rows the page shows, and whether more follow.
    pub fn cut<T>(self, mut rows: Vec<T>) -> (Vec<T>, bool) {
        let more = rows.len() > self.limit as usize;
        rows.truncate(self.limit as usize);
        (rows, more)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_clamped_and_knows_when_more_follow() {
        let page = PageQuery::default().page(60);
        assert_eq!((page.offset, page.limit, page.fetch()), (0, 60, 61));
        let page = PageQuery {
            offset: Some(-5),
            limit: Some(1_000_000),
        }
        .page(60);
        assert_eq!((page.offset, page.limit), (0, Page::MOST));
        let page = PageQuery {
            offset: Some(10),
            limit: Some(2),
        }
        .page(60);
        assert_eq!(page.cut(vec![1, 2, 3]), (vec![1, 2], true));
        assert_eq!(page.cut(vec![1, 2]), (vec![1, 2], false));
        assert_eq!(page.cut(Vec::<i32>::new()), (vec![], false));
    }
}
