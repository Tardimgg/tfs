import { Component } from '@angular/core';
import {AuthService} from '../../services/auth.service';
import {CommonModule, Location} from '@angular/common';
import {RouterLink} from '@angular/router';

@Component({
  selector: 'app-user-info',
  imports: [CommonModule, RouterLink],
  templateUrl: './user-info.component.html',
  styleUrl: './user-info.component.css'
})
export class UserInfoComponent {

  userLogin: string = "";

  constructor(private authService: AuthService, private location: Location) {
    this.userLogin = this.authService.getLogin()
    this.authService.loginEmitter.subscribe((v) => {
      this.userLogin = v;
    })
  }

  logout() {
    this.authService.logout()
    this.userLogin = "";

    window.location.reload()

  }
}
