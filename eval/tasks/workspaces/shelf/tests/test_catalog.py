import unittest

from tests.helpers import library


class Search(unittest.TestCase):
    def test_most_shared_words_first(self):
        found = library().catalog.search("quiet garden")
        self.assertEqual([b.title for b in found], ["The Quiet Garden", "Émile and the Quiet Sea"])

    def test_case_and_accents_fold(self):
        found = library().catalog.search("EMILE")
        self.assertEqual([b.isbn for b in found], ["9781861972712"])

    def test_an_empty_query_finds_nothing(self):
        self.assertEqual(library().catalog.search("  "), [])
