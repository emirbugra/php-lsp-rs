<?php
namespace App\Controllers;

class Home
{
    public function index()
    {
        $users = model('UserModel')->findAll();
        route_to('home');
        route_to('hom');
        lang('App.welcome');
        lang('App.welcom');
        view('users/missing');
        return view('users/index', ['users' => $users, 'config' => config('App')]);
    }

    public function users() {}
}
