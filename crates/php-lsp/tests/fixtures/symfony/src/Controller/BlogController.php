<?php
namespace App\Controller;

use Symfony\Component\Routing\Attribute\Route;

#[Route('/blog', name: 'blog_')]
class BlogController
{
    #[Route('/', name: 'index')]
    public function index()
    {
        $this->generateUrl('blog_index');
        $this->redirectToRoute('blog_indx');
        $this->generateUrl('home');
        $this->generateUrl('_wdt');
        return $this->render('blog/index.html.twig');
    }

    #[Route('/{slug}')]
    public function show()
    {
        $this->render('blog/missing.html.twig');
        return $this->generateUrl('');
    }
}
